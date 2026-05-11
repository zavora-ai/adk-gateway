//! Skill and convention file loader with strict/permissive parsing modes.
//!
//! - `.skills/*.skill.md` files are loaded in **strict** mode: YAML frontmatter
//!   must contain `name` and `description`, otherwise the file is skipped with an error log.
//! - Convention files (CLAW.md, AGENTS.md, etc.) are loaded in **permissive** mode:
//!   name/description are derived from the filename when frontmatter is absent.
//! - Invalid YAML frontmatter triggers a warning; the markdown body is still loaded.
//! - A `SkillIndex` is built using content-based hashing for reliable selection and caching.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

/// Default convention file patterns (R20.1).
pub const DEFAULT_CONVENTION_FILES: &[&str] = &[
    "CLAW.md",
    "AGENTS.md",
    "AGENT.md",
    "CLAUDE.md",
    "GEMINI.md",
    "COPILOT.md",
    "SKILLS.md",
    "SOUL.md",
];

/// A loaded skill document with parsed frontmatter and markdown body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillDocument {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    /// The markdown body (everything after frontmatter).
    pub instructions: String,
    /// Content-based hash for cache invalidation.
    pub content_hash: String,
}

/// YAML frontmatter fields we attempt to parse from skill/convention files.
#[derive(Debug, Clone, Default, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<Vec<String>>,
    trigger: Option<String>,
    #[serde(default)]
    references: Vec<String>,
}

/// Content-addressed index of skill documents.
#[derive(Debug, Clone, Default)]
pub struct SkillIndex {
    /// Skills keyed by content hash.
    by_hash: HashMap<String, SkillDocument>,
    /// Maps skill name → content hash for lookup by name.
    #[allow(dead_code)] // Used by get_by_name(); populated during index build
    by_name: HashMap<String, String>,
}

impl SkillIndex {
    /// Look up a skill by its content hash.
    #[allow(dead_code)] // Used in tests; available for cache-based skill lookup
    pub fn get_by_hash(&self, hash: &str) -> Option<&SkillDocument> {
        self.by_hash.get(hash)
    }

    /// Look up a skill by name.
    #[allow(dead_code)] // Used in tests; available for name-based skill lookup
    pub fn get_by_name(&self, name: &str) -> Option<&SkillDocument> {
        self.by_name.get(name).and_then(|h| self.by_hash.get(h))
    }

    /// Return all skill documents.
    pub fn all(&self) -> impl Iterator<Item = &SkillDocument> {
        self.by_hash.values()
    }

    /// Number of skills in the index.
    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    /// Whether the index is empty.
    #[allow(dead_code)] // Used in tests; available for index status checks
    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }
}

/// Loads skill files and convention files, builds a `SkillIndex`.
pub struct SkillLoader;

impl SkillLoader {
    /// Load `.skill.md` files from a directory in **strict** mode (R21.1–R21.3).
    ///
    /// Requires `name` and `description` in YAML frontmatter.
    /// Files missing either field are logged as errors and skipped.
    pub fn load_skills(dir: &Path) -> Vec<SkillDocument> {
        let mut docs = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "cannot read skills directory");
                return docs;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".skill.md") {
                continue;
            }

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(file = %path.display(), error = %e, "failed to read skill file");
                    continue;
                }
            };

            match parse_skill_strict(&content, &path) {
                Some(doc) => docs.push(doc),
                None => { /* already logged */ }
            }
        }
        docs
    }

    /// Load convention files from a directory in **permissive** mode (R20.1–R20.3, R21.4).
    ///
    /// Checks default patterns plus any `extra_patterns`. Name/description are
    /// derived from the filename when frontmatter is absent.
    pub fn load_conventions(dir: &Path, extra_patterns: &[String]) -> Vec<SkillDocument> {
        let mut docs = Vec::new();
        let patterns: Vec<&str> = DEFAULT_CONVENTION_FILES
            .iter()
            .copied()
            .chain(extra_patterns.iter().map(|s| s.as_str()))
            .collect();

        for pattern in &patterns {
            let path = dir.join(pattern);
            if !path.is_file() {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(file = %path.display(), error = %e, "failed to read convention file");
                    continue;
                }
            };
            docs.push(parse_skill_permissive(&content, &path));
        }
        docs
    }

    /// Convenience method for hot-reload: re-scan skills and conventions,
    /// rebuild the `SkillIndex`, and return the new index (R20.8, R21.10).
    ///
    /// This is the single entry-point that `ConfigWatcher` (Phase 11) and
    /// `.skills/` directory watchers will call when a reload is triggered.
    /// The actual file-system watching trigger will be wired in Phase 11.
    pub fn reload_skills(
        skills_dir: &Path,
        workspace_dir: &Path,
        extra_patterns: &[String],
    ) -> SkillIndex {
        let skills = Self::load_skills(skills_dir);
        let conventions = Self::load_conventions(workspace_dir, extra_patterns);

        tracing::info!(
            skills_count = skills.len(),
            conventions_count = conventions.len(),
            "skill hot-reload: loaded {} skills and {} conventions",
            skills.len(),
            conventions.len(),
        );

        let mut all = skills;
        all.extend(conventions);
        Self::build_index(all)
    }

    /// Build a `SkillIndex` from a collection of skill documents (R21.5).
    pub fn build_index(skills: Vec<SkillDocument>) -> SkillIndex {
        let mut by_hash = HashMap::new();
        let mut by_name = HashMap::new();
        for doc in skills {
            by_name.insert(doc.name.clone(), doc.content_hash.clone());
            by_hash.insert(doc.content_hash.clone(), doc);
        }
        SkillIndex { by_hash, by_name }
    }
}

// ── Parsing helpers ────────────────────────────────────────────────

/// Split a file into optional YAML frontmatter and the markdown body.
/// Frontmatter is delimited by `---` on its own line at the start of the file.
fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content);
    }
    // Find the closing `---`
    let after_open = &trimmed[3..];
    // Skip the rest of the opening line (e.g. trailing whitespace / newline)
    let after_open = after_open
        .strip_prefix('\n')
        .unwrap_or(after_open.strip_prefix("\r\n").unwrap_or(after_open));
    if let Some(close_pos) = after_open.find("\n---") {
        let yaml = &after_open[..close_pos];
        let rest_start = close_pos + 4; // skip "\n---"
        let body = if rest_start < after_open.len() {
            // skip optional newline after closing ---
            let rest = &after_open[rest_start..];
            rest.strip_prefix('\n')
                .or_else(|| rest.strip_prefix("\r\n"))
                .unwrap_or(rest)
        } else {
            ""
        };
        (Some(yaml), body)
    } else {
        // No closing delimiter — treat entire content as body (no frontmatter)
        (None, content)
    }
}

/// Compute a content-based hash string for a document.
fn content_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Derive a human-readable name from a file path (stem without extension).
fn name_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        // Strip .skill suffix if present (e.g. "foo.skill" from "foo.skill.md")
        .strip_suffix(".skill")
        .unwrap_or(
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown"),
        )
        .to_string()
}

/// Parse a `.skill.md` file in **strict** mode.
/// Returns `None` (and logs an error) if `name` or `description` is missing.
fn parse_skill_strict(content: &str, path: &Path) -> Option<SkillDocument> {
    let (yaml_str, body) = split_frontmatter(content);

    let fm: Frontmatter = match yaml_str {
        Some(y) => match serde_json::from_str::<Frontmatter>(
            // serde_yaml is not in deps; use a minimal YAML subset parser
            &yaml_to_json_minimal(y),
        ) {
            Ok(fm) => fm,
            Err(_) => {
                // Try the simple key-value parser
                parse_simple_yaml(y)
            }
        },
        None => {
            tracing::error!(
                file = %path.display(),
                "skill file missing YAML frontmatter — skipping (strict mode)"
            );
            return None;
        }
    };

    let name = match fm.name {
        Some(ref n) if !n.is_empty() => n.clone(),
        _ => {
            tracing::error!(
                file = %path.display(),
                "skill file missing required 'name' in frontmatter — skipping"
            );
            return None;
        }
    };

    let description = match fm.description {
        Some(ref d) if !d.is_empty() => d.clone(),
        _ => {
            tracing::error!(
                file = %path.display(),
                "skill file missing required 'description' in frontmatter — skipping"
            );
            return None;
        }
    };

    Some(SkillDocument {
        name,
        description,
        version: fm.version,
        tags: fm.tags,
        allowed_tools: fm.allowed_tools,
        trigger: fm.trigger,
        references: fm.references,
        instructions: body.to_string(),
        content_hash: content_hash(content),
    })
}

/// Parse a convention file in **permissive** mode.
/// Derives name/description from filename when frontmatter is absent or incomplete.
/// Invalid YAML frontmatter triggers a warning; the body is still loaded (R20.7).
fn parse_skill_permissive(content: &str, path: &Path) -> SkillDocument {
    let (yaml_str, body) = split_frontmatter(content);
    let derived_name = name_from_path(path);

    let fm: Frontmatter = match yaml_str {
        Some(y) => match serde_json::from_str::<Frontmatter>(&yaml_to_json_minimal(y)) {
            Ok(fm) => fm,
            Err(_) => match try_parse_yaml(y) {
                Some(fm) => fm,
                None => {
                    tracing::warn!(
                        file = %path.display(),
                        "invalid YAML frontmatter in convention file — loading body only"
                    );
                    Frontmatter::default()
                }
            },
        },
        None => Frontmatter::default(),
    };

    SkillDocument {
        name: fm.name.unwrap_or_else(|| derived_name.clone()),
        description: fm
            .description
            .unwrap_or_else(|| format!("Convention file: {derived_name}")),
        version: fm.version,
        tags: fm.tags,
        allowed_tools: fm.allowed_tools,
        trigger: fm.trigger,
        references: fm.references,
        instructions: body.to_string(),
        content_hash: content_hash(content),
    }
}

// ── Minimal YAML frontmatter parsing ───────────────────────────────
//
// We avoid adding a full serde_yaml dependency. Skill frontmatter is simple
// key-value pairs (possibly with list values). This parser handles the subset
// we need: `key: value` and `key: [item1, item2]` / multi-line `- item`.

/// Attempt to convert simple YAML key-value pairs into a JSON string
/// that serde_json can parse into `Frontmatter`.
fn yaml_to_json_minimal(yaml: &str) -> String {
    let mut map: Vec<(String, String)> = Vec::new();
    let mut current_key: Option<String> = None;
    let mut list_items: Vec<String> = Vec::new();
    let mut in_list = false;

    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Check for list item under current key
        if in_list && trimmed.starts_with("- ") {
            let val = trimmed[2..].trim().trim_matches('"').trim_matches('\'');
            list_items.push(val.to_string());
            continue;
        }

        // Flush previous list if we were collecting one
        if in_list {
            if let Some(ref key) = current_key {
                let json_arr = format!(
                    "[{}]",
                    list_items
                        .iter()
                        .map(|v| format!("\"{}\"", v.replace('\"', "\\\"")))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                map.push((key.clone(), json_arr));
            }
            list_items.clear();
            in_list = false;
            current_key = None;
        }

        // Parse key: value
        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim().to_string();
            let val = trimmed[colon_pos + 1..].trim();

            if val.is_empty() {
                // Might be start of a list
                current_key = Some(key);
                in_list = true;
                continue;
            }

            // Inline list: [a, b, c]
            if val.starts_with('[') && val.ends_with(']') {
                let inner = &val[1..val.len() - 1];
                let items: Vec<String> = inner
                    .split(',')
                    .map(|s| {
                        let s = s.trim().trim_matches('"').trim_matches('\'');
                        format!("\"{}\"", s.replace('\"', "\\\""))
                    })
                    .collect();
                map.push((key, format!("[{}]", items.join(","))));
            } else {
                // Scalar value
                let val = val.trim_matches('"').trim_matches('\'');
                map.push((key, format!("\"{}\"", val.replace('\"', "\\\""))));
            }
        }
    }

    // Flush trailing list
    if in_list {
        if let Some(ref key) = current_key {
            let json_arr = format!(
                "[{}]",
                list_items
                    .iter()
                    .map(|v| format!("\"{}\"", v.replace('\"', "\\\"")))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            map.push((key.clone(), json_arr));
        }
    }

    let entries: Vec<String> = map
        .iter()
        .map(|(k, v)| format!("\"{}\":{}", k.replace('\"', "\\\""), v))
        .collect();
    format!("{{{}}}", entries.join(","))
}

/// Try parsing YAML via the simple parser, returning None on failure.
fn try_parse_yaml(yaml: &str) -> Option<Frontmatter> {
    let json = yaml_to_json_minimal(yaml);
    serde_json::from_str(&json).ok()
}

/// Parse simple YAML into Frontmatter using the minimal parser.
fn parse_simple_yaml(yaml: &str) -> Frontmatter {
    try_parse_yaml(yaml).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ── Frontmatter splitting ──────────────────────────────────────

    #[test]
    fn test_split_frontmatter_present() {
        let content = "---\nname: test\ndescription: hello\n---\n# Body\nSome text";
        let (yaml, body) = split_frontmatter(content);
        assert!(yaml.is_some());
        assert!(yaml.unwrap().contains("name: test"));
        assert!(body.contains("# Body"));
    }

    #[test]
    fn test_split_frontmatter_absent() {
        let content = "# Just markdown\nNo frontmatter here.";
        let (yaml, body) = split_frontmatter(content);
        assert!(yaml.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn test_split_frontmatter_no_closing() {
        let content = "---\nname: test\nNo closing delimiter";
        let (yaml, body) = split_frontmatter(content);
        assert!(yaml.is_none());
        assert_eq!(body, content);
    }

    // ── Content hashing ────────────────────────────────────────────

    #[test]
    fn test_content_hash_deterministic() {
        let a = content_hash("hello world");
        let b = content_hash("hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn test_content_hash_differs() {
        let a = content_hash("hello");
        let b = content_hash("world");
        assert_ne!(a, b);
    }

    // ── Name derivation ────────────────────────────────────────────

    #[test]
    fn test_name_from_path_skill() {
        let p = PathBuf::from("/skills/coding.skill.md");
        assert_eq!(name_from_path(&p), "coding");
    }

    #[test]
    fn test_name_from_path_convention() {
        let p = PathBuf::from("/workspace/CLAW.md");
        assert_eq!(name_from_path(&p), "CLAW");
    }

    // ── Strict mode (skill files) ──────────────────────────────────

    #[test]
    fn test_strict_mode_valid_skill() {
        let content = "---\nname: code-review\ndescription: Reviews code\nversion: 1.0\n---\nYou are a code reviewer.";
        let path = PathBuf::from("test.skill.md");
        let doc = parse_skill_strict(content, &path).unwrap();
        assert_eq!(doc.name, "code-review");
        assert_eq!(doc.description, "Reviews code");
        assert_eq!(doc.version.as_deref(), Some("1.0"));
        assert!(doc.instructions.contains("code reviewer"));
    }

    #[test]
    fn test_strict_mode_missing_name() {
        let content = "---\ndescription: Reviews code\n---\nBody";
        let path = PathBuf::from("test.skill.md");
        assert!(parse_skill_strict(content, &path).is_none());
    }

    #[test]
    fn test_strict_mode_missing_description() {
        let content = "---\nname: test\n---\nBody";
        let path = PathBuf::from("test.skill.md");
        assert!(parse_skill_strict(content, &path).is_none());
    }

    #[test]
    fn test_strict_mode_no_frontmatter() {
        let content = "# Just markdown";
        let path = PathBuf::from("test.skill.md");
        assert!(parse_skill_strict(content, &path).is_none());
    }

    #[test]
    fn test_strict_mode_with_tags_and_tools() {
        let content = "---\nname: writer\ndescription: Writes prose\ntags: [writing, creative]\nallowed-tools: [web_search, url_context]\n---\nWrite well.";
        let path = PathBuf::from("writer.skill.md");
        let doc = parse_skill_strict(content, &path).unwrap();
        assert_eq!(doc.tags, vec!["writing", "creative"]);
        assert_eq!(
            doc.allowed_tools.as_deref(),
            Some(&["web_search".to_string(), "url_context".to_string()][..])
        );
    }

    // ── Permissive mode (convention files) ─────────────────────────

    #[test]
    fn test_permissive_mode_no_frontmatter() {
        let content = "# Project Instructions\nBe helpful.";
        let path = PathBuf::from("/workspace/CLAW.md");
        let doc = parse_skill_permissive(content, &path);
        assert_eq!(doc.name, "CLAW");
        assert!(doc.description.contains("CLAW"));
        assert!(doc.instructions.contains("Be helpful"));
    }

    #[test]
    fn test_permissive_mode_with_frontmatter() {
        let content = "---\nname: my-project\ndescription: Custom instructions\n---\nDo things.";
        let path = PathBuf::from("/workspace/AGENTS.md");
        let doc = parse_skill_permissive(content, &path);
        assert_eq!(doc.name, "my-project");
        assert_eq!(doc.description, "Custom instructions");
    }

    #[test]
    fn test_permissive_mode_invalid_frontmatter() {
        // Invalid YAML: missing colons, random text
        let content = "---\nthis is not valid yaml {{{}\n---\nStill load this body.";
        let path = PathBuf::from("/workspace/CLAUDE.md");
        let doc = parse_skill_permissive(content, &path);
        // Should derive name from filename and still load body
        assert_eq!(doc.name, "CLAUDE");
        assert!(doc.instructions.contains("Still load this body"));
    }

    // ── SkillIndex ─────────────────────────────────────────────────

    #[test]
    fn test_build_index_lookup_by_name() {
        let doc = SkillDocument {
            name: "test-skill".into(),
            description: "A test".into(),
            version: None,
            tags: vec![],
            allowed_tools: None,
            trigger: None,
            references: vec![],
            instructions: "Do stuff".into(),
            content_hash: "abc123".into(),
        };
        let index = SkillLoader::build_index(vec![doc.clone()]);
        assert_eq!(index.len(), 1);
        assert!(!index.is_empty());
        let found = index.get_by_name("test-skill").unwrap();
        assert_eq!(found.description, "A test");
    }

    #[test]
    fn test_build_index_lookup_by_hash() {
        let doc = SkillDocument {
            name: "x".into(),
            description: "y".into(),
            version: None,
            tags: vec![],
            allowed_tools: None,
            trigger: None,
            references: vec![],
            instructions: "z".into(),
            content_hash: "hash42".into(),
        };
        let index = SkillLoader::build_index(vec![doc]);
        assert!(index.get_by_hash("hash42").is_some());
        assert!(index.get_by_hash("nonexistent").is_none());
    }

    #[test]
    fn test_build_index_empty() {
        let index = SkillLoader::build_index(vec![]);
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
    }

    // ── Filesystem integration (load_skills / load_conventions) ────

    #[test]
    fn test_load_skills_strict_from_dir() {
        let dir = TempDir::new().unwrap();
        // Valid skill
        fs::write(
            dir.path().join("coding.skill.md"),
            "---\nname: coding\ndescription: Code helper\n---\nHelp with code.",
        )
        .unwrap();
        // Invalid skill (missing description)
        fs::write(
            dir.path().join("bad.skill.md"),
            "---\nname: bad\n---\nNo description.",
        )
        .unwrap();
        // Non-skill file (should be ignored)
        fs::write(dir.path().join("README.md"), "# Readme").unwrap();

        let skills = SkillLoader::load_skills(dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "coding");
    }

    #[test]
    fn test_load_conventions_from_dir() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("CLAW.md"), "# Project rules\nBe nice.").unwrap();
        fs::write(
            dir.path().join("AGENTS.md"),
            "---\nname: agents\ndescription: Agent config\n---\nAgent instructions.",
        )
        .unwrap();
        // Not a default pattern
        fs::write(dir.path().join("RANDOM.md"), "Should not load").unwrap();

        let docs = SkillLoader::load_conventions(dir.path(), &[]);
        assert_eq!(docs.len(), 2);
        let names: Vec<&str> = docs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"CLAW"));
        assert!(names.contains(&"agents"));
    }

    #[test]
    fn test_load_conventions_with_extra_patterns() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("CUSTOM.md"), "Custom instructions").unwrap();

        let docs = SkillLoader::load_conventions(dir.path(), &["CUSTOM.md".to_string()]);
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].name, "CUSTOM");
    }

    #[test]
    fn test_load_skills_nonexistent_dir() {
        let skills = SkillLoader::load_skills(Path::new("/nonexistent/path/skills"));
        assert!(skills.is_empty());
    }

    // ── End-to-end: load + build index ─────────────────────────────

    #[test]
    fn test_end_to_end_index_building() {
        let skills_dir = TempDir::new().unwrap();
        let workspace_dir = TempDir::new().unwrap();

        fs::write(
            skills_dir.path().join("writer.skill.md"),
            "---\nname: writer\ndescription: Creative writing\ntrigger: '@writer'\n---\nWrite creatively.",
        )
        .unwrap();
        fs::write(workspace_dir.path().join("CLAW.md"), "Be concise.").unwrap();

        let mut all = SkillLoader::load_skills(skills_dir.path());
        all.extend(SkillLoader::load_conventions(workspace_dir.path(), &[]));

        let index = SkillLoader::build_index(all);
        assert_eq!(index.len(), 2);
        let writer = index.get_by_name("writer").unwrap();
        assert_eq!(writer.trigger.as_deref(), Some("@writer"));
        assert!(index.get_by_name("CLAW").is_some());
    }

    // ── YAML parsing edge cases ────────────────────────────────────

    #[test]
    fn test_yaml_with_trigger_and_references() {
        let content = "---\nname: helper\ndescription: Helps\ntrigger: '@helper'\nreferences:\n- data.json\n- config.csv\n---\nInstructions here.";
        let path = PathBuf::from("helper.skill.md");
        let doc = parse_skill_strict(content, &path).unwrap();
        assert_eq!(doc.trigger.as_deref(), Some("@helper"));
        assert_eq!(doc.references, vec!["data.json", "config.csv"]);
    }

    // ── reload_skills (R20.8, R21.10) ──────────────────────────────

    #[test]
    fn test_reload_skills_combines_skills_and_conventions() {
        let skills_dir = TempDir::new().unwrap();
        let workspace_dir = TempDir::new().unwrap();

        // Create two skill files
        fs::write(
            skills_dir.path().join("alpha.skill.md"),
            "---\nname: alpha\ndescription: Alpha skill\n---\nAlpha instructions.",
        )
        .unwrap();
        fs::write(
            skills_dir.path().join("beta.skill.md"),
            "---\nname: beta\ndescription: Beta skill\n---\nBeta instructions.",
        )
        .unwrap();

        // Create a convention file
        fs::write(
            workspace_dir.path().join("CLAW.md"),
            "# Project conventions\nBe concise.",
        )
        .unwrap();

        // Create a custom convention file
        fs::write(
            workspace_dir.path().join("CUSTOM.md"),
            "Custom convention content.",
        )
        .unwrap();

        let index = SkillLoader::reload_skills(
            skills_dir.path(),
            workspace_dir.path(),
            &["CUSTOM.md".to_string()],
        );

        // 2 skills + 1 default convention (CLAW.md) + 1 extra convention (CUSTOM.md) = 4
        assert_eq!(index.len(), 4);
        assert!(index.get_by_name("alpha").is_some());
        assert!(index.get_by_name("beta").is_some());
        assert!(index.get_by_name("CLAW").is_some());
        assert!(index.get_by_name("CUSTOM").is_some());

        // Verify content is correct
        let alpha = index.get_by_name("alpha").unwrap();
        assert_eq!(alpha.description, "Alpha skill");
        assert!(alpha.instructions.contains("Alpha instructions"));
    }
}
