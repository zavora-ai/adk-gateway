//! Context coordinator for skill-based tool filtering and prompt injection.
//!
//! When a skill is selected via [`SkillIndex`], the [`ContextCoordinator`] filters
//! the agent's available tools to only those in the skill's `allowed_tools` list,
//! injects matched skill instructions into the agent prompt, and loads references
//! from frontmatter into the agent context.
//!
//! Trigger handling: skills with a `trigger` field (e.g. `@writer`) activate only
//! on explicit invocation, not via automatic selection.

use crate::skill_loader::{SkillDocument, SkillIndex};
use crate::tool_registry::ToolEntry;

/// Built context for an agent turn with an active skill.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillContext {
    /// Skill instructions prepended to the user message.
    pub instructions: String,
    /// Names of tools the agent is allowed to use for this turn.
    pub filtered_tool_names: Vec<String>,
    /// Reference file paths from the skill's frontmatter.
    pub references: Vec<String>,
}

/// Coordinates skill selection, tool filtering, and context building.
///
/// Given a [`SkillIndex`], the coordinator selects the best matching skill
/// for a user message, filters the agent's tools per the skill's `allowed_tools`,
/// and builds a [`SkillContext`] with instructions and references.
pub struct ContextCoordinator;

impl ContextCoordinator {
    /// Select the best matching skill for a user message.
    ///
    /// Selection rules:
    /// 1. If the message contains a trigger (e.g. `@writer`), select the skill
    ///    with that trigger — trigger-based skills are *only* activated this way.
    /// 2. Otherwise, pick the first non-trigger skill whose name or tags appear
    ///    in the message (simple keyword match). Skills with a `trigger` field
    ///    are excluded from auto-selection (R21.9, R20.6).
    /// 3. If no skill matches, return `None`.
    pub fn select_skill<'a>(
        user_message: &str,
        available_skills: &'a SkillIndex,
    ) -> Option<&'a SkillDocument> {
        let msg_lower = user_message.to_lowercase();

        // Phase 1: Check for explicit trigger invocations.
        for skill in available_skills.all() {
            if let Some(ref trigger) = skill.trigger {
                let trigger_lower = trigger.to_lowercase();
                if msg_lower.contains(&trigger_lower) {
                    return Some(skill);
                }
            }
        }

        // Phase 2: Auto-select among non-trigger skills by name/tag match.
        for skill in available_skills.all() {
            // Skip trigger-only skills from auto-selection.
            if skill.trigger.is_some() {
                continue;
            }

            let name_lower = skill.name.to_lowercase();
            if msg_lower.contains(&name_lower) {
                return Some(skill);
            }

            for tag in &skill.tags {
                if msg_lower.contains(&tag.to_lowercase()) {
                    return Some(skill);
                }
            }
        }

        None
    }

    /// Filter tools to only those allowed by the active skill.
    ///
    /// If the skill's `allowed_tools` is `None`, all tools are returned (no restriction).
    /// Otherwise, only tools whose names appear in the allowed list are kept (R21.7).
    pub fn filter_tools(tools: &[ToolEntry], skill: &SkillDocument) -> Vec<ToolEntry> {
        match &skill.allowed_tools {
            None => tools.to_vec(),
            Some(allowed) => tools
                .iter()
                .filter(|t| allowed.iter().any(|a| a == &t.name))
                .cloned()
                .collect(),
        }
    }

    /// Build a [`SkillContext`] for the active skill.
    ///
    /// The context includes:
    /// - `instructions`: the skill's markdown body prepended before the user message
    /// - `filtered_tool_names`: names from the skill's `allowed_tools` (or empty if unrestricted)
    /// - `references`: file paths from the skill's frontmatter `references` field
    pub fn build_context(skill: &SkillDocument, user_message: &str) -> SkillContext {
        let instructions = if skill.instructions.is_empty() {
            user_message.to_string()
        } else {
            format!("{}\n\n{}", skill.instructions.trim(), user_message)
        };

        let filtered_tool_names = skill.allowed_tools.clone().unwrap_or_default();

        SkillContext {
            instructions,
            filtered_tool_names,
            references: skill.references.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_loader::SkillLoader;

    fn make_skill(
        name: &str,
        description: &str,
        trigger: Option<&str>,
        allowed_tools: Option<Vec<&str>>,
        tags: Vec<&str>,
        references: Vec<&str>,
        instructions: &str,
    ) -> SkillDocument {
        SkillDocument {
            name: name.to_string(),
            description: description.to_string(),
            version: None,
            tags: tags.into_iter().map(String::from).collect(),
            allowed_tools: allowed_tools.map(|v| v.into_iter().map(String::from).collect()),
            trigger: trigger.map(String::from),
            references: references.into_iter().map(String::from).collect(),
            instructions: instructions.to_string(),
            content_hash: format!("hash_{name}"),
        }
    }

    fn make_tool(name: &str) -> ToolEntry {
        ToolEntry::new(name, format!("{name} tool"), None)
    }

    // ── select_skill: trigger-based activation ─────────────────────

    #[test]
    fn select_skill_matches_trigger() {
        let skill = make_skill(
            "writer",
            "Creative writing",
            Some("@writer"),
            None,
            vec![],
            vec![],
            "Write creatively.",
        );
        let index = SkillLoader::build_index(vec![skill]);

        let result = ContextCoordinator::select_skill("@writer help me write a poem", &index);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "writer");
    }

    #[test]
    fn select_skill_trigger_case_insensitive() {
        let skill = make_skill(
            "writer",
            "Creative writing",
            Some("@Writer"),
            None,
            vec![],
            vec![],
            "Write.",
        );
        let index = SkillLoader::build_index(vec![skill]);

        let result = ContextCoordinator::select_skill("@writer help", &index);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "writer");
    }

    #[test]
    fn select_skill_trigger_in_middle_of_message() {
        let skill = make_skill(
            "coder",
            "Code helper",
            Some("@coder"),
            None,
            vec![],
            vec![],
            "Code.",
        );
        let index = SkillLoader::build_index(vec![skill]);

        let result = ContextCoordinator::select_skill("Hey @coder can you fix this bug?", &index);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "coder");
    }

    // ── select_skill: trigger skills excluded from auto-selection ──

    #[test]
    fn select_skill_trigger_skill_not_auto_selected() {
        let skill = make_skill(
            "writer",
            "Creative writing",
            Some("@writer"),
            None,
            vec!["writing"],
            vec![],
            "Write.",
        );
        let index = SkillLoader::build_index(vec![skill]);

        // Message mentions "writing" tag but doesn't contain the trigger
        let result = ContextCoordinator::select_skill("I need help with writing an essay", &index);
        assert!(result.is_none());
    }

    // ── select_skill: auto-selection by name ───────────────────────

    #[test]
    fn select_skill_auto_by_name() {
        let skill = make_skill(
            "coding",
            "Code helper",
            None,
            None,
            vec![],
            vec![],
            "Help with code.",
        );
        let index = SkillLoader::build_index(vec![skill]);

        let result = ContextCoordinator::select_skill("I need coding help", &index);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "coding");
    }

    // ── select_skill: auto-selection by tag ────────────────────────

    #[test]
    fn select_skill_auto_by_tag() {
        let skill = make_skill(
            "prose-helper",
            "Helps with prose",
            None,
            None,
            vec!["writing", "creative"],
            vec![],
            "Write well.",
        );
        let index = SkillLoader::build_index(vec![skill]);

        let result = ContextCoordinator::select_skill("I need creative assistance", &index);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "prose-helper");
    }

    // ── select_skill: no match ─────────────────────────────────────

    #[test]
    fn select_skill_no_match() {
        let skill = make_skill(
            "coding",
            "Code helper",
            None,
            None,
            vec!["rust", "python"],
            vec![],
            "Code.",
        );
        let index = SkillLoader::build_index(vec![skill]);

        let result = ContextCoordinator::select_skill("Tell me about the weather", &index);
        assert!(result.is_none());
    }

    #[test]
    fn select_skill_empty_index() {
        let index = SkillLoader::build_index(vec![]);
        let result = ContextCoordinator::select_skill("hello", &index);
        assert!(result.is_none());
    }

    // ── select_skill: trigger takes priority over auto ─────────────

    #[test]
    fn select_skill_trigger_priority_over_auto() {
        let trigger_skill = make_skill(
            "writer",
            "Trigger-based writer",
            Some("@writer"),
            None,
            vec![],
            vec![],
            "Triggered.",
        );
        let auto_skill = make_skill(
            "auto-writer",
            "Auto writer",
            None,
            None,
            vec!["writer"],
            vec![],
            "Auto.",
        );
        let index = SkillLoader::build_index(vec![trigger_skill, auto_skill]);

        // Message contains both the trigger and the tag "writer"
        let result = ContextCoordinator::select_skill("@writer write something", &index);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "writer");
    }

    // ── filter_tools ───────────────────────────────────────────────

    #[test]
    fn filter_tools_with_allowed_list() {
        let tools = vec![
            make_tool("web_search"),
            make_tool("code_execution"),
            make_tool("url_context"),
        ];
        let skill = make_skill(
            "researcher",
            "Research skill",
            None,
            Some(vec!["web_search", "url_context"]),
            vec![],
            vec![],
            "Research.",
        );

        let filtered = ContextCoordinator::filter_tools(&tools, &skill);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name, "web_search");
        assert_eq!(filtered[1].name, "url_context");
    }

    #[test]
    fn filter_tools_none_means_all() {
        let tools = vec![make_tool("web_search"), make_tool("code_execution")];
        let skill = make_skill(
            "general",
            "General skill",
            None,
            None,
            vec![],
            vec![],
            "General.",
        );

        let filtered = ContextCoordinator::filter_tools(&tools, &skill);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_tools_empty_allowed_list() {
        let tools = vec![make_tool("web_search"), make_tool("code_execution")];
        let skill = make_skill(
            "restricted",
            "No tools",
            None,
            Some(vec![]),
            vec![],
            vec![],
            "No tools allowed.",
        );

        let filtered = ContextCoordinator::filter_tools(&tools, &skill);
        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_tools_no_matching_tools() {
        let tools = vec![make_tool("web_search")];
        let skill = make_skill(
            "coder",
            "Code only",
            None,
            Some(vec!["code_execution"]),
            vec![],
            vec![],
            "Code.",
        );

        let filtered = ContextCoordinator::filter_tools(&tools, &skill);
        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_tools_empty_tools_list() {
        let skill = make_skill(
            "test",
            "Test",
            None,
            Some(vec!["web_search"]),
            vec![],
            vec![],
            "Test.",
        );

        let filtered = ContextCoordinator::filter_tools(&[], &skill);
        assert!(filtered.is_empty());
    }

    // ── build_context ──────────────────────────────────────────────

    #[test]
    fn build_context_prepends_instructions() {
        let skill = make_skill(
            "writer",
            "Writer",
            None,
            Some(vec!["web_search"]),
            vec![],
            vec!["data.json", "config.csv"],
            "You are a creative writer.",
        );

        let ctx = ContextCoordinator::build_context(&skill, "Write a poem about rust");
        assert!(ctx.instructions.starts_with("You are a creative writer."));
        assert!(ctx.instructions.ends_with("Write a poem about rust"));
        assert!(ctx.instructions.contains("\n\n"));
    }

    #[test]
    fn build_context_empty_instructions() {
        let skill = make_skill("minimal", "Minimal", None, None, vec![], vec![], "");

        let ctx = ContextCoordinator::build_context(&skill, "Hello");
        assert_eq!(ctx.instructions, "Hello");
    }

    #[test]
    fn build_context_includes_references() {
        let skill = make_skill(
            "data-skill",
            "Data",
            None,
            None,
            vec![],
            vec!["schema.json", "data.csv", "notes.md"],
            "Use the data.",
        );

        let ctx = ContextCoordinator::build_context(&skill, "Analyze this");
        assert_eq!(ctx.references, vec!["schema.json", "data.csv", "notes.md"]);
    }

    #[test]
    fn build_context_filtered_tool_names_from_allowed() {
        let skill = make_skill(
            "coder",
            "Coder",
            None,
            Some(vec!["code_execution", "python_code"]),
            vec![],
            vec![],
            "Code.",
        );

        let ctx = ContextCoordinator::build_context(&skill, "Fix this bug");
        assert_eq!(
            ctx.filtered_tool_names,
            vec!["code_execution", "python_code"]
        );
    }

    #[test]
    fn build_context_no_allowed_tools_means_empty_names() {
        let skill = make_skill("general", "General", None, None, vec![], vec![], "General.");

        let ctx = ContextCoordinator::build_context(&skill, "Hello");
        assert!(ctx.filtered_tool_names.is_empty());
    }

    // ── Integration: select + filter + build ───────────────────────

    #[test]
    fn end_to_end_select_filter_build() {
        let skill = make_skill(
            "researcher",
            "Research assistant",
            Some("@research"),
            Some(vec!["web_search", "url_context"]),
            vec![],
            vec!["sources.json"],
            "You are a research assistant. Find accurate information.",
        );
        let index = SkillLoader::build_index(vec![skill]);

        let tools = vec![
            make_tool("web_search"),
            make_tool("code_execution"),
            make_tool("url_context"),
            make_tool("python_code"),
        ];

        let msg = "@research What is the population of Tokyo?";

        // Step 1: Select skill
        let selected = ContextCoordinator::select_skill(msg, &index).unwrap();
        assert_eq!(selected.name, "researcher");

        // Step 2: Filter tools
        let filtered = ContextCoordinator::filter_tools(&tools, selected);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name, "web_search");
        assert_eq!(filtered[1].name, "url_context");

        // Step 3: Build context
        let ctx = ContextCoordinator::build_context(selected, msg);
        assert!(ctx.instructions.contains("research assistant"));
        assert!(ctx.instructions.contains("population of Tokyo"));
        assert_eq!(ctx.filtered_tool_names, vec!["web_search", "url_context"]);
        assert_eq!(ctx.references, vec!["sources.json"]);
    }
}
