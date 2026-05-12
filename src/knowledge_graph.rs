//! Self-contained knowledge graph for per-user entity/relation/observation storage.
//!
//! Implements the 9 KG operations required by R24:
//! - create_entities, create_relations, add_observations
//! - delete_entities, delete_observations, delete_relations
//! - search_nodes, open_nodes, read_graph
//!
//! All operations are scoped by `user_id` for isolation (R24.15).
//! Thread-safe via `DashMap`.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ── Data Types ─────────────────────────────────────────────────────

/// A primary node in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entity {
    pub name: String,
    pub entity_type: String,
    pub observations: Vec<Observation>,
}

/// An atomic piece of information attached to an entity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub id: u64,
    pub content: String,
}

/// A directed edge between two entities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Relation {
    pub id: u64,
    pub source: String,
    pub relation_type: String,
    pub target: String,
}

/// Input for creating a new entity.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateEntityInput {
    pub name: String,
    pub entity_type: String,
    #[serde(default)]
    pub observations: Vec<String>,
}

/// Input for creating a new relation.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateRelationInput {
    pub source: String,
    pub relation_type: String,
    pub target: String,
}

/// Result of a search_nodes query.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub entity: Entity,
    pub relations: Vec<Relation>,
    pub score: f64,
}

/// Per-user graph store.
#[derive(Debug, Default)]
struct UserGraph {
    entities: DashMap<String, Entity>,
    relations: DashMap<u64, Relation>,
}

// ── KnowledgeGraph ─────────────────────────────────────────────────

/// Thread-safe, per-user knowledge graph store.
#[derive(Debug)]
pub struct KnowledgeGraph {
    graphs: DashMap<String, Arc<UserGraph>>,
    next_id: AtomicU64,
}

impl Default for KnowledgeGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self {
            graphs: DashMap::new(),
            next_id: AtomicU64::new(1),
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn user_graph(&self, user_id: &str) -> Arc<UserGraph> {
        self.graphs
            .entry(user_id.to_string())
            .or_insert_with(|| Arc::new(UserGraph::default()))
            .clone()
    }

    // ── Create Operations ──────────────────────────────────────────

    /// Create entities in the user's knowledge graph (R24.5).
    /// If an entity with the same name already exists, its observations are merged.
    pub fn create_entities(&self, user_id: &str, inputs: Vec<CreateEntityInput>) -> Vec<String> {
        let graph = self.user_graph(user_id);
        let mut created = Vec::new();

        for input in inputs {
            let observations: Vec<Observation> = input
                .observations
                .into_iter()
                .map(|content| Observation {
                    id: self.next_id(),
                    content,
                })
                .collect();

            graph
                .entities
                .entry(input.name.clone())
                .and_modify(|existing| {
                    existing.observations.extend(observations.clone());
                })
                .or_insert_with(|| Entity {
                    name: input.name.clone(),
                    entity_type: input.entity_type,
                    observations,
                });

            created.push(input.name);
        }

        created
    }

    /// Create relations in the user's knowledge graph (R24.6).
    pub fn create_relations(&self, user_id: &str, inputs: Vec<CreateRelationInput>) -> Vec<u64> {
        let graph = self.user_graph(user_id);
        let mut ids = Vec::new();

        for input in inputs {
            let id = self.next_id();
            graph.relations.insert(
                id,
                Relation {
                    id,
                    source: input.source,
                    relation_type: input.relation_type,
                    target: input.target,
                },
            );
            ids.push(id);
        }

        ids
    }

    /// Append observations to an existing entity (R24.7).
    /// Returns the IDs of the created observations, or None if entity not found.
    pub fn add_observations(
        &self,
        user_id: &str,
        entity_name: &str,
        contents: Vec<String>,
    ) -> Option<Vec<u64>> {
        let graph = self.user_graph(user_id);
        let mut entry = graph.entities.get_mut(entity_name)?;
        let mut ids = Vec::new();

        for content in contents {
            let id = self.next_id();
            entry.observations.push(Observation { id, content });
            ids.push(id);
        }

        Some(ids)
    }

    // ── Delete Operations ──────────────────────────────────────────

    /// Delete entities and all their associated relations (R24.8).
    #[allow(dead_code)] // Registered as kg_delete_entities tool; used in tests
    pub fn delete_entities(&self, user_id: &str, names: Vec<String>) -> Vec<String> {
        let graph = self.user_graph(user_id);
        let mut deleted = Vec::new();

        for name in names {
            if graph.entities.remove(&name).is_some() {
                // Remove all relations involving this entity (inbound + outbound)
                let to_remove: Vec<u64> = graph
                    .relations
                    .iter()
                    .filter(|r| r.source == name || r.target == name)
                    .map(|r| r.id)
                    .collect();
                for rid in to_remove {
                    graph.relations.remove(&rid);
                }
                deleted.push(name);
            }
        }

        deleted
    }

    /// Delete specific observations by ID (R24.9).
    #[allow(dead_code)] // Registered as kg_delete_observations tool; used in tests
    pub fn delete_observations(&self, user_id: &str, observation_ids: Vec<u64>) -> Vec<u64> {
        let graph = self.user_graph(user_id);
        let mut deleted = Vec::new();
        let id_set: std::collections::HashSet<u64> = observation_ids.into_iter().collect();

        for mut entry in graph.entities.iter_mut() {
            let before = entry.observations.len();
            entry.observations.retain(|o| !id_set.contains(&o.id));
            let removed = before - entry.observations.len();
            if removed > 0 {
                deleted.extend(
                    id_set
                        .iter()
                        .filter(|id| !entry.observations.iter().any(|o| o.id == **id))
                        .copied(),
                );
            }
        }

        // Deduplicate (an ID can only belong to one entity)
        deleted.sort();
        deleted.dedup();
        deleted
    }

    /// Delete specific relations by ID (R24.10).
    #[allow(dead_code)] // Registered as kg_delete_relations tool; used in tests
    pub fn delete_relations(&self, user_id: &str, relation_ids: Vec<u64>) -> Vec<u64> {
        let graph = self.user_graph(user_id);
        let mut deleted = Vec::new();

        for id in relation_ids {
            if graph.relations.remove(&id).is_some() {
                deleted.push(id);
            }
        }

        deleted
    }

    // ── Query Operations ───────────────────────────────────────────

    /// Simple text-matching search across entity names, types, and observations (R24.11).
    /// Returns matching entities with their relations, ranked by match count.
    pub fn search_nodes(&self, user_id: &str, query: &str) -> Vec<SearchResult> {
        let graph = self.user_graph(user_id);
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for entry in graph.entities.iter() {
            let entity = entry.value();
            let mut score: f64 = 0.0;

            // Match against entity name
            if entity.name.to_lowercase().contains(&query_lower) {
                score += 3.0;
            }

            // Match against entity type
            if entity.entity_type.to_lowercase().contains(&query_lower) {
                score += 2.0;
            }

            // Match against observations
            for obs in &entity.observations {
                if obs.content.to_lowercase().contains(&query_lower) {
                    score += 1.0;
                }
            }

            if score > 0.0 {
                let relations: Vec<Relation> = graph
                    .relations
                    .iter()
                    .filter(|r| r.source == entity.name || r.target == entity.name)
                    .map(|r| r.value().clone())
                    .collect();

                results.push(SearchResult {
                    entity: entity.clone(),
                    relations,
                    score,
                });
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    /// Return full details for named entities (R24.12).
    #[allow(dead_code)] // Registered as kg_open_nodes tool; used in tests
    pub fn open_nodes(&self, user_id: &str, names: Vec<String>) -> Vec<SearchResult> {
        let graph = self.user_graph(user_id);
        let mut results = Vec::new();

        for name in names {
            if let Some(entity) = graph.entities.get(&name) {
                let relations: Vec<Relation> = graph
                    .relations
                    .iter()
                    .filter(|r| r.source == name || r.target == name)
                    .map(|r| r.value().clone())
                    .collect();

                results.push(SearchResult {
                    entity: entity.clone(),
                    relations,
                    score: 1.0,
                });
            }
        }

        results
    }

    /// Return all entities and relations for the user (R24.13).
    #[allow(dead_code)] // Registered as kg_read_graph tool; used in tests
    pub fn read_graph(&self, user_id: &str) -> (Vec<Entity>, Vec<Relation>) {
        let graph = self.user_graph(user_id);

        let entities: Vec<Entity> = graph.entities.iter().map(|e| e.value().clone()).collect();
        let relations: Vec<Relation> = graph.relations.iter().map(|r| r.value().clone()).collect();

        (entities, relations)
    }

    /// Return all user IDs that have data in the knowledge graph.
    pub fn user_ids(&self) -> Vec<String> {
        self.graphs.iter().map(|entry| entry.key().clone()).collect()
    }

    /// Delete the entire knowledge graph for a user (R24.19).
    pub fn delete_user_graph(&self, user_id: &str) -> bool {
        self.graphs.remove(user_id).is_some()
    }

    /// Build a compact summary of all entities for a user.
    ///
    /// This produces an always-available memory snapshot that can be injected
    /// into every prompt, giving the agent full awareness of stored knowledge
    /// without requiring a search query to match.
    pub fn build_entity_summary(&self, user_id: &str, max_obs_per_entity: usize) -> String {
        let graph = self.user_graph(user_id);
        if graph.entities.is_empty() {
            return String::new();
        }

        let mut summary = String::from("[Active memory — known entities and facts]\n");

        // Collect and sort entities for deterministic output
        let mut entities: Vec<Entity> = graph.entities.iter().map(|e| e.value().clone()).collect();
        entities.sort_by(|a, b| a.name.cmp(&b.name));

        for entity in &entities {
            summary.push_str(&format!("• {} ({})", entity.name, entity.entity_type));
            if !entity.observations.is_empty() {
                // Take the most recent observations (last N)
                let obs: Vec<&str> = entity
                    .observations
                    .iter()
                    .rev()
                    .take(max_obs_per_entity)
                    .map(|o| o.content.as_str())
                    .collect();
                summary.push_str(": ");
                summary.push_str(&obs.into_iter().rev().collect::<Vec<_>>().join("; "));
            }
            summary.push('\n');
        }

        // Add relations
        let relations: Vec<Relation> = graph.relations.iter().map(|r| r.value().clone()).collect();
        if !relations.is_empty() {
            summary.push_str("Relationships:\n");
            for rel in &relations {
                summary.push_str(&format!(
                    "  {} —[{}]→ {}\n",
                    rel.source, rel.relation_type, rel.target
                ));
            }
        }

        summary
    }

    /// Trim observations per entity to keep only the most recent N.
    /// Returns the number of observations removed.
    pub fn trim_observations(&self, user_id: &str, max_per_entity: usize) -> usize {
        let graph = self.user_graph(user_id);
        let mut removed = 0;
        for mut entry in graph.entities.iter_mut() {
            let len = entry.observations.len();
            if len > max_per_entity {
                let excess = len - max_per_entity;
                entry.observations.drain(0..excess);
                removed += excess;
            }
        }
        removed
    }
}

// ── KnowledgeGraphToolset ──────────────────────────────────────────

/// High-level toolset that wraps `KnowledgeGraph` and scopes all
/// operations to a specific user_id. This is the struct that gets
/// registered with each agent when memory is configured (R24.4).
pub struct KnowledgeGraphToolset {
    #[allow(dead_code)] // Accessed via graph() method; used in tests
    graph: Arc<KnowledgeGraph>,
}

impl KnowledgeGraphToolset {
    pub fn new(graph: Arc<KnowledgeGraph>) -> Self {
        Self { graph }
    }

    #[allow(dead_code)] // Used in tests for direct graph access
    pub fn graph(&self) -> &Arc<KnowledgeGraph> {
        &self.graph
    }

    #[allow(dead_code)] // Delegates to KnowledgeGraph; registered as agent tool
    pub fn create_entities(&self, user_id: &str, inputs: Vec<CreateEntityInput>) -> Vec<String> {
        self.graph.create_entities(user_id, inputs)
    }

    #[allow(dead_code)] // Delegates to KnowledgeGraph; registered as agent tool
    pub fn create_relations(&self, user_id: &str, inputs: Vec<CreateRelationInput>) -> Vec<u64> {
        self.graph.create_relations(user_id, inputs)
    }

    #[allow(dead_code)] // Delegates to KnowledgeGraph; registered as agent tool
    pub fn add_observations(
        &self,
        user_id: &str,
        entity_name: &str,
        contents: Vec<String>,
    ) -> Option<Vec<u64>> {
        self.graph.add_observations(user_id, entity_name, contents)
    }

    #[allow(dead_code)] // Delegates to KnowledgeGraph; registered as agent tool
    pub fn delete_entities(&self, user_id: &str, names: Vec<String>) -> Vec<String> {
        self.graph.delete_entities(user_id, names)
    }

    #[allow(dead_code)] // Delegates to KnowledgeGraph; registered as agent tool
    pub fn delete_observations(&self, user_id: &str, observation_ids: Vec<u64>) -> Vec<u64> {
        self.graph.delete_observations(user_id, observation_ids)
    }

    #[allow(dead_code)] // Delegates to KnowledgeGraph; registered as agent tool
    pub fn delete_relations(&self, user_id: &str, relation_ids: Vec<u64>) -> Vec<u64> {
        self.graph.delete_relations(user_id, relation_ids)
    }

    #[allow(dead_code)] // Delegates to KnowledgeGraph; registered as agent tool
    pub fn search_nodes(&self, user_id: &str, query: &str) -> Vec<SearchResult> {
        self.graph.search_nodes(user_id, query)
    }

    #[allow(dead_code)] // Delegates to KnowledgeGraph; registered as agent tool
    pub fn open_nodes(&self, user_id: &str, names: Vec<String>) -> Vec<SearchResult> {
        self.graph.open_nodes(user_id, names)
    }

    #[allow(dead_code)] // Delegates to KnowledgeGraph; registered as agent tool
    pub fn read_graph(&self, user_id: &str) -> (Vec<Entity>, Vec<Relation>) {
        self.graph.read_graph(user_id)
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_graph() -> KnowledgeGraph {
        KnowledgeGraph::new()
    }

    fn entity_input(name: &str, etype: &str, obs: Vec<&str>) -> CreateEntityInput {
        CreateEntityInput {
            name: name.to_string(),
            entity_type: etype.to_string(),
            observations: obs.into_iter().map(String::from).collect(),
        }
    }

    fn relation_input(source: &str, rtype: &str, target: &str) -> CreateRelationInput {
        CreateRelationInput {
            source: source.to_string(),
            relation_type: rtype.to_string(),
            target: target.to_string(),
        }
    }

    #[test]
    fn test_create_and_read_entities() {
        let kg = make_graph();
        let created = kg.create_entities(
            "user1",
            vec![
                entity_input("Alice", "person", vec!["works at Acme", "likes Rust"]),
                entity_input("Acme", "organization", vec!["tech company"]),
            ],
        );
        assert_eq!(created, vec!["Alice", "Acme"]);

        let (entities, _) = kg.read_graph("user1");
        assert_eq!(entities.len(), 2);

        let alice = entities.iter().find(|e| e.name == "Alice").unwrap();
        assert_eq!(alice.entity_type, "person");
        assert_eq!(alice.observations.len(), 2);
    }

    #[test]
    fn test_create_relations() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![
                entity_input("Alice", "person", vec![]),
                entity_input("Acme", "organization", vec![]),
            ],
        );

        let ids = kg.create_relations("user1", vec![relation_input("Alice", "works_at", "Acme")]);
        assert_eq!(ids.len(), 1);

        let (_, relations) = kg.read_graph("user1");
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].source, "Alice");
        assert_eq!(relations[0].relation_type, "works_at");
        assert_eq!(relations[0].target, "Acme");
    }

    #[test]
    fn test_add_observations() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![entity_input("Alice", "person", vec!["age 30"])],
        );

        let obs_ids = kg
            .add_observations("user1", "Alice", vec!["likes coffee".into()])
            .unwrap();
        assert_eq!(obs_ids.len(), 1);

        let results = kg.open_nodes("user1", vec!["Alice".into()]);
        assert_eq!(results[0].entity.observations.len(), 2);
    }

    #[test]
    fn test_add_observations_missing_entity() {
        let kg = make_graph();
        let result = kg.add_observations("user1", "NonExistent", vec!["fact".into()]);
        assert!(result.is_none());
    }

    #[test]
    fn test_delete_entities_cascades_relations() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![
                entity_input("Alice", "person", vec![]),
                entity_input("Bob", "person", vec![]),
            ],
        );
        kg.create_relations("user1", vec![relation_input("Alice", "knows", "Bob")]);

        let deleted = kg.delete_entities("user1", vec!["Alice".into()]);
        assert_eq!(deleted, vec!["Alice"]);

        let (entities, relations) = kg.read_graph("user1");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "Bob");
        assert_eq!(relations.len(), 0); // relation was cascaded
    }

    #[test]
    fn test_delete_observations() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![entity_input(
                "Alice",
                "person",
                vec!["fact1", "fact2", "fact3"],
            )],
        );

        let alice = kg.open_nodes("user1", vec!["Alice".into()]);
        let obs_id = alice[0].entity.observations[1].id; // "fact2"

        let deleted = kg.delete_observations("user1", vec![obs_id]);
        assert_eq!(deleted.len(), 1);

        let alice = kg.open_nodes("user1", vec!["Alice".into()]);
        assert_eq!(alice[0].entity.observations.len(), 2);
        assert!(alice[0]
            .entity
            .observations
            .iter()
            .all(|o| o.content != "fact2"));
    }

    #[test]
    fn test_delete_relations() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![
                entity_input("A", "node", vec![]),
                entity_input("B", "node", vec![]),
            ],
        );
        let ids = kg.create_relations(
            "user1",
            vec![
                relation_input("A", "links_to", "B"),
                relation_input("B", "links_to", "A"),
            ],
        );

        let deleted = kg.delete_relations("user1", vec![ids[0]]);
        assert_eq!(deleted, vec![ids[0]]);

        let (_, relations) = kg.read_graph("user1");
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].id, ids[1]);
    }

    #[test]
    fn test_search_nodes() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![
                entity_input("Alice", "person", vec!["works at Acme", "likes Rust"]),
                entity_input("Acme", "organization", vec!["tech company"]),
                entity_input("Bob", "person", vec!["works at Globex"]),
            ],
        );

        let results = kg.search_nodes("user1", "Acme");
        assert!(results.len() >= 2); // Alice (observation match) + Acme (name match)
                                     // Acme should rank higher (name match = 3.0) than Alice (observation match = 1.0)
        assert_eq!(results[0].entity.name, "Acme");
    }

    #[test]
    fn test_search_nodes_no_match() {
        let kg = make_graph();
        kg.create_entities("user1", vec![entity_input("Alice", "person", vec![])]);

        let results = kg.search_nodes("user1", "zzz_no_match");
        assert!(results.is_empty());
    }

    #[test]
    fn test_open_nodes() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![
                entity_input("Alice", "person", vec!["fact1"]),
                entity_input("Bob", "person", vec!["fact2"]),
            ],
        );
        kg.create_relations("user1", vec![relation_input("Alice", "knows", "Bob")]);

        let results = kg.open_nodes("user1", vec!["Alice".into()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entity.name, "Alice");
        assert_eq!(results[0].relations.len(), 1);
    }

    #[test]
    fn test_open_nodes_missing() {
        let kg = make_graph();
        let results = kg.open_nodes("user1", vec!["NonExistent".into()]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_read_graph_empty() {
        let kg = make_graph();
        let (entities, relations) = kg.read_graph("user1");
        assert!(entities.is_empty());
        assert!(relations.is_empty());
    }

    #[test]
    fn test_user_isolation() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![entity_input("Alice", "person", vec!["user1 data"])],
        );
        kg.create_entities(
            "user2",
            vec![entity_input("Bob", "person", vec!["user2 data"])],
        );

        let (entities1, _) = kg.read_graph("user1");
        let (entities2, _) = kg.read_graph("user2");

        assert_eq!(entities1.len(), 1);
        assert_eq!(entities1[0].name, "Alice");

        assert_eq!(entities2.len(), 1);
        assert_eq!(entities2[0].name, "Bob");

        // Search should be isolated too
        let results = kg.search_nodes("user1", "Bob");
        assert!(results.is_empty());
    }

    #[test]
    fn test_delete_user_graph() {
        let kg = make_graph();
        kg.create_entities("user1", vec![entity_input("Alice", "person", vec!["fact"])]);
        kg.create_relations("user1", vec![relation_input("Alice", "self", "Alice")]);

        assert!(kg.delete_user_graph("user1"));
        let (entities, relations) = kg.read_graph("user1");
        assert!(entities.is_empty());
        assert!(relations.is_empty());
    }

    #[test]
    fn test_delete_user_graph_nonexistent() {
        let kg = make_graph();
        assert!(!kg.delete_user_graph("nobody"));
    }

    #[test]
    fn test_create_entity_merge_observations() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![entity_input("Alice", "person", vec!["fact1"])],
        );
        kg.create_entities(
            "user1",
            vec![entity_input("Alice", "person", vec!["fact2"])],
        );

        let results = kg.open_nodes("user1", vec!["Alice".into()]);
        assert_eq!(results[0].entity.observations.len(), 2);
    }

    #[test]
    fn test_toolset_delegates_to_graph() {
        let graph = Arc::new(KnowledgeGraph::new());
        let toolset = KnowledgeGraphToolset::new(graph);

        toolset.create_entities("u1", vec![entity_input("X", "thing", vec!["obs"])]);
        let (entities, _) = toolset.read_graph("u1");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "X");
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let kg = Arc::new(KnowledgeGraph::new());
        let mut handles = vec![];

        for i in 0..10 {
            let kg = kg.clone();
            handles.push(thread::spawn(move || {
                let user = format!("user_{i}");
                kg.create_entities(
                    &user,
                    vec![entity_input(&format!("Entity_{i}"), "test", vec!["data"])],
                );
                let (entities, _) = kg.read_graph(&user);
                assert_eq!(entities.len(), 1);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn test_build_entity_summary_empty() {
        let kg = make_graph();
        let summary = kg.build_entity_summary("user1", 5);
        assert!(summary.is_empty());
    }

    #[test]
    fn test_build_entity_summary_with_entities() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![
                entity_input("Alice", "identity", vec!["name is Alice", "likes Rust"]),
                entity_input("Acme", "context", vec!["tech company", "works here"]),
            ],
        );
        kg.create_relations("user1", vec![relation_input("Alice", "works_at", "Acme")]);

        let summary = kg.build_entity_summary("user1", 10);
        assert!(summary.contains("[Active memory"));
        assert!(summary.contains("Alice (identity)"));
        assert!(summary.contains("Acme (context)"));
        assert!(summary.contains("name is Alice"));
        assert!(summary.contains("works_at"));
    }

    #[test]
    fn test_build_entity_summary_caps_observations() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![entity_input(
                "Alice",
                "identity",
                vec!["fact1", "fact2", "fact3", "fact4", "fact5"],
            )],
        );

        // Only show last 2 observations
        let summary = kg.build_entity_summary("user1", 2);
        assert!(summary.contains("fact4"));
        assert!(summary.contains("fact5"));
        assert!(!summary.contains("fact1"));
    }

    #[test]
    fn test_trim_observations() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![entity_input(
                "Alice",
                "identity",
                vec!["old1", "old2", "old3", "recent1", "recent2"],
            )],
        );

        let removed = kg.trim_observations("user1", 3);
        assert_eq!(removed, 2);

        let results = kg.open_nodes("user1", vec!["Alice".into()]);
        let obs: Vec<&str> = results[0]
            .entity
            .observations
            .iter()
            .map(|o| o.content.as_str())
            .collect();
        assert_eq!(obs, vec!["old3", "recent1", "recent2"]);
    }

    #[test]
    fn test_trim_observations_no_excess() {
        let kg = make_graph();
        kg.create_entities(
            "user1",
            vec![entity_input("Alice", "identity", vec!["fact1", "fact2"])],
        );

        let removed = kg.trim_observations("user1", 10);
        assert_eq!(removed, 0);
    }
}
