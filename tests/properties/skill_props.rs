//! Property-based tests for skills and conventions.
//!
//! Feature: gateway-production-maturity
//! - Property 8: Skill loading respects strict vs permissive mode
//! - Property 9: Convention file discovery covers all default patterns
//! - Property 10: Trigger field prevents auto-activation
//! - Property 11: ContextCoordinator filters tools per active skill
//! - Property 36: Invalid convention file frontmatter loads body
//! **Validates: R4.1, R20.1-R20.7, R21.1-R21.4, R21.7, R21.9**

use adk_gateway::context_coordinator::ContextCoordinator;
use adk_gateway::skill_loader::{SkillDocument, SkillLoader, DEFAULT_CONVENTION_FILES};
use adk_gateway::tool_registry::ToolEntry;
use proptest::prelude::*;
use std::collections::HashSet;
use tempfile::TempDir;

/// Strategy for generating valid skill names (alphanumeric + hyphens, non-empty).
fn arb_skill_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9\\-]{0,15}"
}

/// Strategy for generating non-empty skill descriptions.
fn arb_skill_description() -> impl Strategy<Value = String> {
    "[A-Za-z ]{3,40}"
}

/// Strategy for generating markdown body content.
fn arb_markdown_body() -> impl Strategy<Value = String> {
    "[A-Za-z0-9 \\.\\-]{1,80}"
}

/// Strategy for generating tool names.
fn arb_tool_name() -> impl Strategy<Value = String> {
    "[a-z_]{2,16}"
}

// ── Property 8: Skill loading respects strict vs permissive mode ───
// **Validates: Requirements R21.1, R21.2, R21.3, R21.4, R20.2, R20.3**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    #[test]
    fn skill_loading_strict_vs_permissive(
        name in arb_skill_name(),
        description in arb_skill_description(),
        body in arb_markdown_body(),
    ) {
        let dir = TempDir::new().unwrap();

        // ── Strict mode: valid skill file with name + description → loads ──
        let valid_content = format!(
            "---\nname: {name}\ndescription: {description}\n---\n{body}"
        );
        std::fs::write(
            dir.path().join("valid.skill.md"),
            &valid_content,
        ).unwrap();

        // ── Strict mode: missing description → skipped ──
        let no_desc = format!("---\nname: {name}\n---\nSome body");
        std::fs::write(
            dir.path().join("no-desc.skill.md"),
            &no_desc,
        ).unwrap();

        // ── Strict mode: missing name → skipped ──
        let no_name = format!("---\ndescription: {description}\n---\nSome body");
        std::fs::write(
            dir.path().join("no-name.skill.md"),
            &no_name,
        ).unwrap();

        // ── Strict mode: no frontmatter at all → skipped ──
        std::fs::write(
            dir.path().join("bare.skill.md"),
            &body,
        ).unwrap();

        let skills = SkillLoader::load_skills(dir.path());

        // Only the valid skill should load (R21.2, R21.3)
        prop_assert_eq!(
            skills.len(), 1,
            "only the valid skill file should load in strict mode, got {}",
            skills.len()
        );
        prop_assert_eq!(&skills[0].name, &name.trim().to_string());
        // YAML parsing trims trailing whitespace from values
        prop_assert_eq!(&skills[0].description, &description.trim().to_string());
        prop_assert!(
            skills[0].instructions.contains(body.trim()),
            "skill body should contain the markdown content"
        );

        // ── Permissive mode: convention file without frontmatter → loads with derived name ──
        let conv_dir = TempDir::new().unwrap();
        std::fs::write(
            conv_dir.path().join("CLAW.md"),
            &body,
        ).unwrap();

        let conventions = SkillLoader::load_conventions(conv_dir.path(), &[]);
        prop_assert_eq!(
            conventions.len(), 1,
            "convention file should load in permissive mode"
        );
        // Name derived from filename (R20.2, R21.4)
        prop_assert_eq!(&conventions[0].name, "CLAW");
        prop_assert!(
            conventions[0].instructions.contains(&body),
            "convention body should contain the markdown content"
        );
    }
}

// ── Property 9: Convention file discovery covers all default patterns ──
// **Validates: Requirements R20.1**
#[test]
fn convention_file_discovery_covers_all_default_patterns() {
    let expected: HashSet<&str> = [
        "CLAW.md",
        "AGENTS.md",
        "AGENT.md",
        "CLAUDE.md",
        "GEMINI.md",
        "COPILOT.md",
        "SKILLS.md",
        "SOUL.md",
    ]
    .into_iter()
    .collect();

    let actual: HashSet<&str> = DEFAULT_CONVENTION_FILES.iter().copied().collect();

    assert_eq!(
        actual,
        expected,
        "DEFAULT_CONVENTION_FILES should contain exactly the expected patterns.\n\
         Missing: {:?}\n\
         Extra: {:?}",
        expected.difference(&actual).collect::<Vec<_>>(),
        actual.difference(&expected).collect::<Vec<_>>(),
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20))]

    /// Verify that each default convention file, when present on disk, is discovered.
    #[test]
    fn convention_discovery_finds_each_default_file(
        body in arb_markdown_body(),
        idx in 0..DEFAULT_CONVENTION_FILES.len(),
    ) {
        let dir = TempDir::new().unwrap();
        let filename = DEFAULT_CONVENTION_FILES[idx];
        std::fs::write(dir.path().join(filename), &body).unwrap();

        let docs = SkillLoader::load_conventions(dir.path(), &[]);
        prop_assert_eq!(
            docs.len(), 1,
            "convention file '{}' should be discovered", filename
        );
        prop_assert!(
            docs[0].instructions.contains(&body),
            "convention body should match written content"
        );
    }
}

// ── Property 10: Trigger field prevents auto-activation ──
// **Validates: Requirements R21.9, R20.6**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(40))]

    #[test]
    fn trigger_field_prevents_auto_activation(
        skill_name in arb_skill_name(),
        trigger in "@[a-z]{3,10}",
        // Message that does NOT contain the trigger
        unrelated_msg in "[A-Z][a-z ]{5,30}",
    ) {
        let skill = SkillDocument {
            name: skill_name.clone(),
            description: "A triggered skill".to_string(),
            version: None,
            tags: vec![skill_name.clone()],
            allowed_tools: None,
            trigger: Some(trigger.clone()),
            references: vec![],
            instructions: "Triggered instructions.".to_string(),
            content_hash: format!("hash_{skill_name}"),
        };
        let index = SkillLoader::build_index(vec![skill]);

        // Message without trigger → skill NOT selected (R21.9)
        let result = ContextCoordinator::select_skill(&unrelated_msg, &index);
        prop_assert!(
            result.is_none(),
            "skill with trigger '{}' should NOT be auto-selected for message '{}'",
            trigger, unrelated_msg
        );

        // Message WITH trigger → skill IS selected
        let msg_with_trigger = format!("Hey {} can you help?", trigger);
        let result = ContextCoordinator::select_skill(&msg_with_trigger, &index);
        prop_assert!(
            result.is_some(),
            "skill with trigger '{}' should be selected when message contains it",
            trigger
        );
        prop_assert_eq!(
            &result.unwrap().name, &skill_name,
            "selected skill name should match"
        );
    }
}

// ── Property 11: ContextCoordinator filters tools per active skill ──
// **Validates: Requirements R21.7**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    #[test]
    fn context_coordinator_filters_tools_per_skill(
        all_tools in prop::collection::vec(arb_tool_name(), 1..10),
        allowed_indices in prop::collection::vec(any::<prop::sample::Index>(), 0..8),
    ) {
        // Deduplicate tool names
        let mut seen = HashSet::new();
        let tools: Vec<ToolEntry> = all_tools
            .iter()
            .filter(|n| seen.insert((*n).clone()))
            .map(|n| ToolEntry::new(n.clone(), format!("{n} description"), None))
            .collect();

        if tools.is_empty() {
            return Ok(());
        }

        // Pick a subset of tool names as the allowed list
        let allowed: Vec<String> = allowed_indices
            .iter()
            .map(|idx| tools[idx.index(tools.len())].name.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        let skill = SkillDocument {
            name: "test-skill".to_string(),
            description: "Test".to_string(),
            version: None,
            tags: vec![],
            allowed_tools: Some(allowed.clone()),
            trigger: None,
            references: vec![],
            instructions: "Test instructions.".to_string(),
            content_hash: "hash_test".to_string(),
        };

        let filtered = ContextCoordinator::filter_tools(&tools, &skill);
        let filtered_names: HashSet<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
        let allowed_set: HashSet<&str> = allowed.iter().map(|s| s.as_str()).collect();

        // Filtered tools should be exactly the intersection of tools and allowed_tools (R21.7)
        for tool in &tools {
            if allowed_set.contains(tool.name.as_str()) {
                prop_assert!(
                    filtered_names.contains(tool.name.as_str()),
                    "tool '{}' is in allowed_tools and should be in filtered result",
                    tool.name
                );
            } else {
                prop_assert!(
                    !filtered_names.contains(tool.name.as_str()),
                    "tool '{}' is NOT in allowed_tools and should NOT be in filtered result",
                    tool.name
                );
            }
        }

        // No extra tools should appear
        for ft in &filtered {
            prop_assert!(
                tools.iter().any(|t| t.name == ft.name),
                "filtered tool '{}' should come from the original tool list",
                ft.name
            );
        }
    }
}

// ── Property 36: Invalid convention file frontmatter loads body ──
// **Validates: Requirements R20.7**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    #[test]
    fn invalid_frontmatter_still_loads_body(
        body in arb_markdown_body(),
        garbage_yaml in "[^\\-]{5,30}\\{\\}\\[",
    ) {
        let dir = TempDir::new().unwrap();
        // Write a convention file with invalid YAML frontmatter
        let content = format!("---\n{garbage_yaml}\n---\n{body}");
        std::fs::write(dir.path().join("CLAW.md"), &content).unwrap();

        let docs = SkillLoader::load_conventions(dir.path(), &[]);
        prop_assert_eq!(
            docs.len(), 1,
            "convention file with invalid frontmatter should still load"
        );

        // Name should be derived from filename since frontmatter is invalid (R20.7)
        prop_assert_eq!(
            &docs[0].name, "CLAW",
            "name should be derived from filename when frontmatter is invalid"
        );

        // Body should still be loaded
        prop_assert!(
            docs[0].instructions.contains(&body),
            "body should be loaded even with invalid frontmatter. Got: '{}'",
            docs[0].instructions
        );
    }
}
