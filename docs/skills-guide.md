# Skills Authoring Guide

## Overview

Skills are markdown files that provide composable agent capabilities. They live in the `.skills/` directory and use YAML frontmatter for metadata.

## Skill Format

```markdown
---
name: code-reviewer
description: Reviews code for best practices and security issues
version: "1.0"
allowed_tools:
  - code_execution
  - web_search
trigger: "@review"
tags: [code, security]
---

# Code Reviewer

You are a code review assistant. When reviewing code:

1. Check for security vulnerabilities
2. Verify error handling
3. Suggest performance improvements
```

### Required Frontmatter (Strict Mode)

Files in `.skills/` **must** have:
- `name` — Unique skill identifier
- `description` — What the skill does

Missing either field causes the skill to be rejected with an error log.

### Optional Frontmatter

- `version` — Semantic version
- `allowed_tools` — List of tools available when this skill is active
- `trigger` — Explicit activation pattern (e.g., `@review`). Prevents auto-activation.
- `tags` — Categorization tags
- `references` — Additional files to load into context (JSON, CSV, etc.)
- `hint` — Short hint for skill selection
- `metadata` — Arbitrary key-value pairs

## Convention Files (Permissive Mode)

Convention files are project-level instructions auto-discovered from the workspace:

| File | Purpose |
|------|---------|
| `CLAW.md` | Primary project instructions (checked first) |
| `AGENTS.md` | Multi-agent configuration |
| `CLAUDE.md` | Claude-specific instructions |
| `GEMINI.md` | Gemini-specific instructions |
| `COPILOT.md` | Copilot-specific instructions |
| `SKILLS.md` | Skills documentation |
| `SOUL.md` | Agent personality/behavioral constraints |

Convention files use **permissive mode**: name and description are derived from the filename when frontmatter is absent. Invalid YAML frontmatter is logged as a warning, and the markdown body is still loaded.

## Custom Patterns

Add custom convention file patterns in config:

```json5
{
  "conventions": {
    "enabled": true,
    "extraPatterns": ["CUSTOM.md", "RULES.md"]
  }
}
```

## Tool Filtering

When a skill with `allowed_tools` is active, only those tools are available to the agent. This enforces per-skill tool governance.

## Hot-Reload

Skills are reloaded when:
- The config file changes
- Files in `.skills/` are added, modified, or removed
