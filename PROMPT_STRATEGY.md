# Prompt Architecture Strategy

This document defines the prompt architecture for Claude Code integrations, providing clear guidance on what instructions belong in each configuration file.

## Overview

Claude Code uses a layered prompt system where instructions flow from multiple sources into the final system prompt. Understanding this architecture is critical for effective prompt engineering—putting instructions in the wrong place leads to confusion, duplication, or instructions being ignored.

```
┌─────────────────────────────────────────────────────────────────┐
│                     Claude Code System Prompt                    │
├─────────────────────────────────────────────────────────────────┤
│  Base Claude Code Instructions (Anthropic-controlled)           │
├─────────────────────────────────────────────────────────────────┤
│  MCP_INSTRUCTIONS.md (per MCP server)                           │
│  └── Server-wide tool guidance                                  │
├─────────────────────────────────────────────────────────────────┤
│  CLAUDE.md (per project)                                        │
│  └── Project-specific context & workflows                       │
├─────────────────────────────────────────────────────────────────┤
│  SKILL.md (per skill, loaded on-demand)                         │
│  └── Skill-specific patterns & tool selection                   │
├─────────────────────────────────────────────────────────────────┤
│  Tool Descriptions (inline with each tool)                      │
│  └── Individual tool documentation                              │
└─────────────────────────────────────────────────────────────────┘
```

---

## File Roles and Scope

### 1. CLAUDE.md — Project Instructions

**Location:** `CLAUDE.md` or `.claude/CLAUDE.md` in project root
**Loaded:** Always, when Claude Code runs in the workspace
**Visibility:** All conversations in this project
**Ownership:** Project maintainers (checked into repo)

**Purpose:** Define project-specific context, workflows, and preferences that apply regardless of what tools or skills are used.

**Should contain:**
- Project architecture overview
- Build/test/run commands
- Development workflow preferences
- Definition of Done criteria
- Release processes
- Project-specific tool selection hints (e.g., "use gabb instead of grep")
- File organization conventions
- Issue/planning workflow preferences
- Code style guidelines not captured by linters

**Should NOT contain:**
- Generic tool documentation (goes in tool descriptions)
- Detailed skill usage patterns (goes in SKILL.md)
- MCP server configuration (goes in MCP_INSTRUCTIONS.md)
- Instructions that apply to all projects (goes in user settings)

**Example content:**
```markdown
## Tool Selection: Use gabb for Code Navigation

This project has gabb MCP tools available. Use them instead of Grep/Read.

| Task | Use gabb | NOT this |
|------|----------|----------|
| Find function | `gabb_symbols` | `Grep "def foo"` |

## Development Workflow

- All planning is done via GitHub Issues
- Run `cargo test` before committing
- Reference issue numbers in commits
```

---

### 2. SKILL.md — Skill-Specific Instructions

**Location:** `.claude/skills/<skill-name>/SKILL.md`
**Loaded:** On-demand when skill is invoked or relevant
**Visibility:** Conversations where skill is active
**Ownership:** Skill authors (may be auto-generated or user-created)

**Purpose:** Provide detailed guidance for a specific capability domain—when to use it, how to use it effectively, and common patterns.

**Should contain:**
- When/why to use this skill vs alternatives
- Tool selection matrix within the skill domain
- Common usage patterns with examples
- Limitations and edge cases
- Supported languages/file types (if applicable)
- Workflow recommendations specific to the skill
- Performance tips
- Fallback strategies when the skill isn't applicable

**Should NOT contain:**
- Project-specific configuration (goes in CLAUDE.md)
- Individual tool parameter documentation (goes in tool descriptions)
- Server-wide MCP guidance (goes in MCP_INSTRUCTIONS.md)
- Information duplicated across multiple skills

**Example content:**
```markdown
# gabb Code Navigation Skill

## When to Use gabb

Use gabb tools instead of Grep/Read/Glob for code navigation in supported languages.

**Use gabb when:**
- Finding symbol definitions (functions, classes, types)
- Tracing call graphs
- Finding all usages before refactoring
- Understanding file structure

**Fall back to Grep/Read when:**
- Searching non-code files (markdown, config)
- Languages not supported by gabb
- Searching for string literals or comments

## Supported Languages

| Language | Extensions | Notes |
|----------|------------|-------|
| Rust | .rs | Full support |
| TypeScript | .ts, .tsx | Full support |

## Common Patterns

### Before reading a large file
```
gabb_structure file="src/large_file.rs"
```
Then use line numbers to read specific sections.
```

---

### 3. Tool Descriptions — Inline Tool Documentation

**Location:** Embedded in MCP tool definitions (description field)
**Loaded:** Always (part of tool schema)
**Visibility:** Available whenever tool is callable
**Ownership:** MCP server authors

**Purpose:** Document what a single tool does, its parameters, and when to use it vs similar tools.

**Should contain:**
- Clear description of what the tool does
- All parameter documentation
- Return value description
- "USE THIS when..." guidance for tool selection
- Brief examples of common invocations
- Relationship to similar tools ("Use X instead when...")

**Should NOT contain:**
- Multi-step workflows (goes in SKILL.md or MCP_INSTRUCTIONS.md)
- Project-specific guidance (goes in CLAUDE.md)
- Server-wide configuration (goes in MCP_INSTRUCTIONS.md)
- Lengthy tutorials (keep descriptions focused)

**Example content:**
```
Find all functions/methods that call a given function/method.

USE THIS when you want to understand who calls a function, trace
execution flow backwards, or assess impact before modifying a function.

Parameters:
- file: Path to file containing the function definition
- line: 1-based line number of the function
- character: 1-based column number
- transitive: Include full call chain (default: false)

Point to a function definition to see all its callers.
Use transitive=true to get the full call chain.
```

---

### 4. MCP_INSTRUCTIONS.md — Server-Wide Guidance

**Location:** Provided by MCP server (embedded in server config)
**Loaded:** When MCP server connects
**Visibility:** All conversations where server is active
**Ownership:** MCP server authors

**Purpose:** Provide cross-cutting guidance that applies to all tools from an MCP server—selection heuristics, workflows, and server-specific patterns.

**Should contain:**
- Tool selection guidance across all server tools
- Cross-tool workflows ("First do X, then Y")
- Context management strategies (pagination, batching)
- Server-specific conventions
- Common multi-tool patterns
- Error handling guidance
- Authentication/permission context

**Should NOT contain:**
- Individual tool documentation (goes in tool descriptions)
- Project-specific workflows (goes in CLAUDE.md)
- Skill-specific patterns (goes in SKILL.md)
- Information that varies by tool

**Example content:**
```markdown
## Tool Selection Guidance

1. Use 'list_*' tools for broad retrieval with basic filtering
2. Use 'search_*' tools for targeted queries with specific criteria

## Context Management

- Use pagination with batches of 5-10 items
- Set minimal_output=true when full info isn't needed

## Pull Request Workflow

1. Create pending review with pull_request_review_write
2. Add comments with add_comment_to_pending_review
3. Submit with pull_request_review_write method=submit_pending
```

---

## Decision Framework

Use this flowchart to decide where instructions belong:

```
Is this about a SPECIFIC TOOL's parameters/behavior?
├── YES → Tool Description
└── NO ↓

Is this about how multiple tools from ONE SERVER work together?
├── YES → MCP_INSTRUCTIONS.md
└── NO ↓

Is this about a SKILL DOMAIN (code nav, git, etc.) across tools?
├── YES → SKILL.md
└── NO ↓

Is this about THIS PROJECT's specific needs?
├── YES → CLAUDE.md
└── NO → Consider if it belongs in user settings or is already
          covered by Claude Code's base instructions
```

### Quick Reference Table

| Instruction Type | Location | Example |
|-----------------|----------|---------|
| "This tool accepts X parameter" | Tool Description | `line: 1-based line number` |
| "Use tool A before tool B" | MCP_INSTRUCTIONS.md | "Create review before adding comments" |
| "For code navigation, prefer X over Y" | SKILL.md | "Use gabb_structure before Read" |
| "In this project, use X workflow" | CLAUDE.md | "Track work via GitHub Issues" |
| "This tool finds symbol definitions" | Tool Description | USE THIS when... |
| "Batch requests to reduce context" | MCP_INSTRUCTIONS.md | "Use pagination with 5-10 items" |
| "Supported languages: Rust, TS" | SKILL.md | Language support table |
| "Run cargo test before commits" | CLAUDE.md | Project workflow |

---

## Anti-Patterns to Avoid

### 1. Duplication Across Files
**Bad:** Listing supported languages in CLAUDE.md, SKILL.md, AND tool descriptions.
**Good:** Single source of truth in SKILL.md, reference from CLAUDE.md if needed.

### 2. Project Config in Tool Descriptions
**Bad:** Tool description says "In gabb-cli, use --workspace ."
**Good:** Tool description is generic; CLAUDE.md specifies project paths.

### 3. Workflows in Tool Descriptions
**Bad:** Tool description contains 5-step workflow using multiple tools.
**Good:** Tool description covers single tool; MCP_INSTRUCTIONS.md covers workflow.

### 4. Generic Guidance in CLAUDE.md
**Bad:** CLAUDE.md explains how grep works.
**Good:** CLAUDE.md says "prefer gabb over grep for code"; SKILL.md explains when/why.

### 5. Overly Long Tool Descriptions
**Bad:** 500-word tool description with tutorials.
**Good:** Focused description with "USE THIS when"; detailed patterns in SKILL.md.

---

## Layering and Override Semantics

When the same topic is addressed in multiple places, more specific sources take precedence:

```
Tool Description < MCP_INSTRUCTIONS.md < SKILL.md < CLAUDE.md
     (generic)                                        (specific)
```

**Example:**
- Tool description says "Use gabb_symbols for finding functions"
- MCP_INSTRUCTIONS.md says "Prefer gabb tools over grep for code"
- SKILL.md says "For files >100 lines, use gabb_structure first"
- CLAUDE.md says "In this project, always use include_source=true"

All four are valid and layer together. CLAUDE.md's project-specific hint adds to (doesn't replace) the skill guidance.

---

## Maintenance Guidelines

### When Adding a New Tool
1. Write focused tool description (what, parameters, when to use)
2. Update MCP_INSTRUCTIONS.md if tool affects cross-tool workflows
3. Update SKILL.md if tool is part of a skill domain
4. Update CLAUDE.md only if project has specific usage patterns

### When Changing Tool Behavior
1. Update tool description first
2. Check if MCP_INSTRUCTIONS.md workflows are affected
3. Check if SKILL.md patterns need updating
4. Notify projects that may have CLAUDE.md overrides

### When Creating a New Skill
1. Create SKILL.md with: purpose, tool selection, patterns, limitations
2. Ensure tool descriptions are self-contained (don't require SKILL.md)
3. Add CLAUDE.md entry only for project-specific needs

### Periodic Review
- Tool descriptions: Review when tool changes
- MCP_INSTRUCTIONS.md: Review when adding/removing tools
- SKILL.md: Review quarterly or when patterns evolve
- CLAUDE.md: Review when project workflows change

---

## Template: SKILL.md Structure

```markdown
# [Skill Name]

## Purpose
One paragraph explaining what this skill enables.

## When to Use
- Bullet points for ideal use cases
- Include "Use this when..." patterns

## When NOT to Use
- Limitations and exclusions
- Fallback recommendations

## Tool Selection
| Task | Tool | Notes |
|------|------|-------|
| ... | ... | ... |

## Supported Languages/Formats
(if applicable)

## Common Patterns

### Pattern Name
Brief description and example.

## Tips and Best Practices
- Performance tips
- Common mistakes to avoid
```

---

## Template: MCP_INSTRUCTIONS.md Structure

```markdown
## [Server Name] MCP Server

Brief description of what this server provides.

## Tool Selection Guidance
How to choose between tools in this server.

## Context Management
Pagination, batching, output size recommendations.

## Common Workflows
Multi-tool patterns with step-by-step guidance.

## Error Handling
How to handle common error scenarios.
```
