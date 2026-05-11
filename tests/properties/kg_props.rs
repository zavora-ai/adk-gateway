//! Property-based tests for knowledge graph operations.
//!
//! Feature: gateway-production-maturity
//! - Property 39: Knowledge graph entity round trip
//!   **Validates: Requirements R24.5, R24.6, R24.7, R24.12, R24.13**
//! - Property 40: Knowledge graph deletion correctness
//!   **Validates: Requirements R24.8, R24.9, R24.10**
//! - Property 41: Knowledge graph user isolation
//!   **Validates: Requirements R24.15**
//! - Property 42: Knowledge graph search returns relevant results
//!   **Validates: Requirements R24.11**

use adk_gateway::knowledge_graph::{CreateEntityInput, CreateRelationInput, KnowledgeGraph};
use proptest::prelude::*;
use std::collections::HashSet;

// ── Strategies ─────────────────────────────────────────────────────

/// Arbitrary non-empty entity name.
fn arb_entity_name() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_]{0,19}".prop_filter("non-empty", |s| !s.is_empty())
}

/// Arbitrary entity type.
fn arb_entity_type() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("person".to_string()),
        Just("organization".to_string()),
        Just("concept".to_string()),
        Just("location".to_string()),
        Just("event".to_string()),
    ]
}

/// Arbitrary observation text.
fn arb_observation() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 ]{1,50}"
}

/// Arbitrary CreateEntityInput with unique name.
fn arb_entity_input() -> impl Strategy<Value = CreateEntityInput> {
    (
        arb_entity_name(),
        arb_entity_type(),
        prop::collection::vec(arb_observation(), 0..4),
    )
        .prop_map(|(name, entity_type, observations)| CreateEntityInput {
            name,
            entity_type,
            observations,
        })
}

/// A set of entity inputs with unique names.
fn arb_unique_entities(
    count: std::ops::Range<usize>,
) -> impl Strategy<Value = Vec<CreateEntityInput>> {
    prop::collection::vec(arb_entity_input(), count).prop_map(|entities| {
        let mut seen = HashSet::new();
        entities
            .into_iter()
            .filter(|e| seen.insert(e.name.clone()))
            .collect()
    })
}

/// Arbitrary user ID.
fn arb_user_id() -> impl Strategy<Value = String> {
    "[a-z]{1,10}"
}

// ── Property 39: Knowledge graph entity round trip ─────────────────
// **Validates: Requirements R24.5, R24.6, R24.7, R24.12, R24.13**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// Property 39: Knowledge graph entity round trip.
    ///
    /// For any set of entities created via create_entities, calling open_nodes
    /// with those entity names returns equivalent entity data. Calling read_graph
    /// includes all created entities and relations for that user.
    #[test]
    fn kg_entity_round_trip(
        entities in arb_unique_entities(1..6),
        user_id in arb_user_id(),
    ) {
        let kg = KnowledgeGraph::new();

        // Create entities
        let names: Vec<String> = entities.iter().map(|e| e.name.clone()).collect();
        let created = kg.create_entities(&user_id, entities.clone());
        prop_assert_eq!(&created, &names, "created names should match input names");

        // Create relations between consecutive entities if we have >= 2
        let mut expected_relation_count = 0;
        if names.len() >= 2 {
            let relation_inputs: Vec<CreateRelationInput> = names
                .windows(2)
                .map(|w| CreateRelationInput {
                    source: w[0].clone(),
                    relation_type: "relates_to".to_string(),
                    target: w[1].clone(),
                })
                .collect();
            expected_relation_count = relation_inputs.len();
            let rel_ids = kg.create_relations(&user_id, relation_inputs);
            prop_assert_eq!(rel_ids.len(), expected_relation_count);
        }

        // open_nodes should return all entities with correct data (R24.12)
        let opened = kg.open_nodes(&user_id, names.clone());
        prop_assert_eq!(
            opened.len(), names.len(),
            "open_nodes should return all {} entities", names.len()
        );

        for entity_input in &entities {
            let found = opened.iter().find(|r| r.entity.name == entity_input.name);
            prop_assert!(found.is_some(), "entity '{}' should be in open_nodes results", entity_input.name);
            let found = found.unwrap();
            prop_assert_eq!(&found.entity.entity_type, &entity_input.entity_type);
            prop_assert_eq!(
                found.entity.observations.len(),
                entity_input.observations.len(),
                "observation count mismatch for '{}'", entity_input.name
            );
            for (obs, expected) in found.entity.observations.iter().zip(&entity_input.observations) {
                prop_assert_eq!(&obs.content, expected);
            }
        }

        // read_graph should include all entities and relations (R24.13)
        let (all_entities, all_relations) = kg.read_graph(&user_id);
        prop_assert_eq!(
            all_entities.len(), names.len(),
            "read_graph should return all {} entities", names.len()
        );
        prop_assert_eq!(
            all_relations.len(), expected_relation_count,
            "read_graph should return all {} relations", expected_relation_count
        );

        // Verify add_observations round trip (R24.7)
        if let Some(first_name) = names.first() {
            let new_obs = vec!["extra observation".to_string()];
            let obs_ids = kg.add_observations(&user_id, first_name, new_obs.clone());
            prop_assert!(obs_ids.is_some(), "add_observations should succeed for existing entity");
            let obs_ids = obs_ids.unwrap();
            prop_assert_eq!(obs_ids.len(), 1);

            let reopened = kg.open_nodes(&user_id, vec![first_name.clone()]);
            let entity = &reopened[0].entity;
            prop_assert!(
                entity.observations.iter().any(|o| o.content == "extra observation"),
                "added observation should appear in open_nodes"
            );
        }
    }
}

// ── Property 40: Knowledge graph deletion correctness ──────────────
// **Validates: Requirements R24.8, R24.9, R24.10**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// Property 40: Knowledge graph deletion correctness.
    ///
    /// Deleting an entity removes it and cascades to its relations.
    /// Deleting an observation removes only that observation.
    /// Deleting a relation removes only that relation.
    #[test]
    fn kg_deletion_correctness(
        entities in arb_unique_entities(2..6),
        user_id in arb_user_id(),
    ) {
        let kg = KnowledgeGraph::new();

        // Create entities with at least one observation each
        let mut inputs: Vec<CreateEntityInput> = entities.into_iter().map(|mut e| {
            if e.observations.is_empty() {
                e.observations.push("default observation".to_string());
            }
            e
        }).collect();

        // Ensure we have at least 2 entities
        if inputs.len() < 2 {
            inputs.push(CreateEntityInput {
                name: "extra_entity".to_string(),
                entity_type: "concept".to_string(),
                observations: vec!["extra obs".to_string()],
            });
        }

        let names: Vec<String> = inputs.iter().map(|e| e.name.clone()).collect();
        kg.create_entities(&user_id, inputs);

        // Create relations between all consecutive pairs
        let relation_inputs: Vec<CreateRelationInput> = names
            .windows(2)
            .map(|w| CreateRelationInput {
                source: w[0].clone(),
                relation_type: "links_to".to_string(),
                target: w[1].clone(),
            })
            .collect();
        let _rel_ids = kg.create_relations(&user_id, relation_inputs);

        // --- Test entity deletion with cascade (R24.8) ---
        let to_delete = names[0].clone();
        let deleted = kg.delete_entities(&user_id, vec![to_delete.clone()]);
        prop_assert_eq!(deleted, vec![to_delete.clone()]);

        // Deleted entity should not appear in read_graph
        let (remaining_entities, remaining_relations) = kg.read_graph(&user_id);
        prop_assert!(
            !remaining_entities.iter().any(|e| e.name == to_delete),
            "deleted entity '{}' should not appear in read_graph", to_delete
        );

        // Relations involving deleted entity should be gone
        prop_assert!(
            !remaining_relations.iter().any(|r| r.source == to_delete || r.target == to_delete),
            "relations involving '{}' should be cascaded on delete", to_delete
        );

        // Remaining entities should be intact
        for name in &names[1..] {
            prop_assert!(
                remaining_entities.iter().any(|e| &e.name == name),
                "remaining entity '{}' should still exist", name
            );
        }

        // Deleted entity should not appear in search or open_nodes
        let search_results = kg.search_nodes(&user_id, &to_delete);
        prop_assert!(
            !search_results.iter().any(|r| r.entity.name == to_delete),
            "deleted entity should not appear in search_nodes"
        );
        let open_results = kg.open_nodes(&user_id, vec![to_delete.clone()]);
        prop_assert!(open_results.is_empty(), "deleted entity should not appear in open_nodes");

        // --- Test observation deletion (R24.9) ---
        let surviving_name = &names[1];
        let opened = kg.open_nodes(&user_id, vec![surviving_name.clone()]);
        if !opened.is_empty() && !opened[0].entity.observations.is_empty() {
            let obs_to_delete = opened[0].entity.observations[0].id;
            let obs_count_before = opened[0].entity.observations.len();

            let deleted_obs = kg.delete_observations(&user_id, vec![obs_to_delete]);
            prop_assert!(deleted_obs.contains(&obs_to_delete));

            let reopened = kg.open_nodes(&user_id, vec![surviving_name.clone()]);
            prop_assert_eq!(
                reopened[0].entity.observations.len(),
                obs_count_before - 1,
                "observation count should decrease by 1"
            );
            prop_assert!(
                !reopened[0].entity.observations.iter().any(|o| o.id == obs_to_delete),
                "deleted observation should not appear"
            );
            // Entity itself should still exist
            prop_assert_eq!(&reopened[0].entity.name, surviving_name);
        }

        // --- Test relation deletion (R24.10) ---
        let (_, current_relations) = kg.read_graph(&user_id);
        if !current_relations.is_empty() {
            let rel_to_delete = current_relations[0].id;
            let rel_count_before = current_relations.len();

            let deleted_rels = kg.delete_relations(&user_id, vec![rel_to_delete]);
            prop_assert!(deleted_rels.contains(&rel_to_delete));

            let (_, after_relations) = kg.read_graph(&user_id);
            prop_assert_eq!(
                after_relations.len(),
                rel_count_before - 1,
                "relation count should decrease by 1"
            );
            prop_assert!(
                !after_relations.iter().any(|r| r.id == rel_to_delete),
                "deleted relation should not appear"
            );
        }
    }
}

// ── Property 41: Knowledge graph user isolation ────────────────────
// **Validates: Requirements R24.15**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// Property 41: Knowledge graph user isolation.
    ///
    /// For any two distinct user_ids, entities created by one user
    /// never appear in query results for the other user.
    #[test]
    fn kg_user_isolation(
        entities_a in arb_unique_entities(1..5),
        entities_b in arb_unique_entities(1..5),
        user_a in "[a-z]{3,8}".prop_map(|s| format!("user_a_{s}")),
        user_b in "[a-z]{3,8}".prop_map(|s| format!("user_b_{s}")),
    ) {
        let kg = KnowledgeGraph::new();

        let names_a: Vec<String> = entities_a.iter().map(|e| e.name.clone()).collect();
        let names_b: Vec<String> = entities_b.iter().map(|e| e.name.clone()).collect();

        // Create entities for user A
        kg.create_entities(&user_a, entities_a);

        // Create entities for user B
        kg.create_entities(&user_b, entities_b);

        // read_graph for user A should only contain user A's entities
        let (graph_a, _) = kg.read_graph(&user_a);
        let graph_a_names: HashSet<String> = graph_a.iter().map(|e| e.name.clone()).collect();
        for name in &names_a {
            prop_assert!(
                graph_a_names.contains(name),
                "user_a's entity '{}' should be in user_a's graph", name
            );
        }
        // user_b's entities that don't share a name with user_a should NOT appear
        for name in &names_b {
            if !names_a.contains(name) {
                prop_assert!(
                    !graph_a_names.contains(name),
                    "user_b's entity '{}' should NOT be in user_a's graph", name
                );
            }
        }

        // read_graph for user B should only contain user B's entities
        let (graph_b, _) = kg.read_graph(&user_b);
        let graph_b_names: HashSet<String> = graph_b.iter().map(|e| e.name.clone()).collect();
        for name in &names_b {
            prop_assert!(
                graph_b_names.contains(name),
                "user_b's entity '{}' should be in user_b's graph", name
            );
        }
        // user_a's entities that don't share a name with user_b should NOT appear
        for name in &names_a {
            if !names_b.contains(name) {
                prop_assert!(
                    !graph_b_names.contains(name),
                    "user_a's entity '{}' should NOT be in user_b's graph", name
                );
            }
        }

        // search_nodes for user A should not return user B's entities
        for name in &names_b {
            let results = kg.search_nodes(&user_a, name);
            prop_assert!(
                !results.iter().any(|r| r.entity.name == *name && names_b.contains(&r.entity.name) && !names_a.contains(&r.entity.name)),
                "search_nodes for user_a should not return user_b's entity '{}'", name
            );
        }

        // open_nodes for user A should not return user B's entities
        let opened_a = kg.open_nodes(&user_a, names_b.clone());
        for result in &opened_a {
            prop_assert!(
                names_a.contains(&result.entity.name),
                "open_nodes for user_a should not return user_b's entity '{}'", result.entity.name
            );
        }
    }
}

// ── Property 42: Knowledge graph search returns relevant results ───
// **Validates: Requirements R24.11**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// Property 42: Knowledge graph search returns relevant results.
    ///
    /// For any entity with known text in observations, searching for that
    /// text returns the matching entity in results, and all results belong
    /// to the querying user.
    #[test]
    fn kg_search_returns_relevant_results(
        entity_name in arb_entity_name(),
        entity_type in arb_entity_type(),
        search_term in "[a-zA-Z]{4,12}",
        user_id in arb_user_id(),
    ) {
        let kg = KnowledgeGraph::new();

        // Create an entity with the search term embedded in an observation
        let observation = format!("this entity is about {search_term} specifically");
        kg.create_entities(&user_id, vec![CreateEntityInput {
            name: entity_name.clone(),
            entity_type: entity_type.clone(),
            observations: vec![observation.clone()],
        }]);

        // Also create a decoy entity with unrelated content
        kg.create_entities(&user_id, vec![CreateEntityInput {
            name: format!("{entity_name}_decoy"),
            entity_type: "concept".to_string(),
            observations: vec!["completely unrelated content xyz123".to_string()],
        }]);

        // Search for the term
        let results = kg.search_nodes(&user_id, &search_term);

        // The entity with the matching observation should appear in results
        prop_assert!(
            results.iter().any(|r| r.entity.name == entity_name),
            "entity '{}' with observation containing '{}' should appear in search results",
            entity_name, search_term
        );

        // All results should have a positive score
        for result in &results {
            prop_assert!(
                result.score > 0.0,
                "search result for '{}' should have positive score, got {}",
                result.entity.name, result.score
            );
        }

        // Results should be sorted by score descending
        for window in results.windows(2) {
            prop_assert!(
                window[0].score >= window[1].score,
                "results should be sorted by score descending: {} >= {}",
                window[0].score, window[1].score
            );
        }

        // Search with a different user should return nothing
        let other_results = kg.search_nodes("nonexistent_user", &search_term);
        prop_assert!(
            other_results.is_empty(),
            "search for a different user should return no results"
        );
    }
}

// ── Property 20: Knowledge graph user isolation (gateway-full-wiring) ──
// Feature: gateway-full-wiring, Property 20: Knowledge graph user isolation
// **Validates: Requirements 15.3**

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 20: For any two distinct user IDs and entities created for each,
    /// search_nodes for user A should never return entities belonging to user B.
    #[test]
    fn kg_user_isolation_search_never_leaks(
        entities_a in arb_unique_entities(1..4),
        entities_b in arb_unique_entities(1..4),
        user_a in "[a-z]{3,8}".prop_map(|s| format!("iso_a_{s}")),
        user_b in "[a-z]{3,8}".prop_map(|s| format!("iso_b_{s}")),
    ) {
        let kg = KnowledgeGraph::new();

        let names_a: Vec<String> = entities_a.iter().map(|e| e.name.clone()).collect();
        let names_b: Vec<String> = entities_b.iter().map(|e| e.name.clone()).collect();

        // Create entities for each user
        kg.create_entities(&user_a, entities_a);
        kg.create_entities(&user_b, entities_b);

        // For every entity name belonging to user B, searching as user A
        // should never return that entity (unless user A also has an entity
        // with the same name, which is fine — it's user A's own entity).
        for name_b in &names_b {
            let results_a = kg.search_nodes(&user_a, name_b);
            for result in &results_a {
                // Every result must belong to user A's entity set
                prop_assert!(
                    names_a.contains(&result.entity.name),
                    "search_nodes for user_a returned entity '{}' which belongs to user_b",
                    result.entity.name
                );
            }
        }

        // Symmetric: search as user B should never return user A's entities
        for name_a in &names_a {
            let results_b = kg.search_nodes(&user_b, name_a);
            for result in &results_b {
                prop_assert!(
                    names_b.contains(&result.entity.name),
                    "search_nodes for user_b returned entity '{}' which belongs to user_a",
                    result.entity.name
                );
            }
        }
    }
}
