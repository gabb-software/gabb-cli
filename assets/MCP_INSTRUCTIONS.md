# Gabb MCP Server

Code indexing server providing lightweight file structure previews via SQLite index.

## Exploration Ladder

Match exploration depth to **actual** difficulty, not predicted difficulty.
Start simple and escalate only when needed.

### Level 1 - Direct (try first)
- Go directly to the most likely file based on task description
- If you find what you need, stop here
- Use when: task names a specific file/function, change is localized, obvious location

### Level 2 - Targeted Search (if Level 1 insufficient)
- Use Grep for specific symbols/patterns mentioned in the task
- Read 1-2 additional files identified by search
- Use when: direct attempt didn't find the target, need to locate a symbol

### Level 3 - Structure Overview (if still stuck)
- Use gabb_structure on candidate files to understand layout
- Read targeted sections based on structure
- Use when: file is large and you need to find the right section

### Level 4 - Full Exploration (only for genuinely complex tasks)
- Use Task agents for systematic codebase exploration
- Use when: architectural questions, multi-component changes, truly unknown territory

**Key principle:** Start at Level 1 and escalate only when needed.
Don't predict difficulty—discover it.

⚠️ **Over-exploration costs time.** A trivial task that could be solved in 15s
can take 60s+ with unnecessary exploration.

## Pre-Read Check for Code Files (Recommended)

For large or unfamiliar code files, consider calling `gabb_structure` first to see the layout.
This saves tokens when you only need part of a large file.

**Recommended for:**
- Large files (>100 lines) where you only need part
- Unfamiliar codebases where you're exploring
- Files you'll read multiple times in a session

**Skip this check when:**
- You already know exactly what you're looking for
- The file is likely small (<100 lines)
- You can answer from existing context
- Files you've already seen structure for

**The pattern:**
```
gabb_structure file="path/to/file.rs"      # ~50 tokens, shows layout
Read file="path/to/file.rs" offset=X limit=Y   # Read only what you need
```

## Supported Languages

| Language   | Extensions                              |
|------------|----------------------------------------|
| Python     | `.py`, `.pyi`                          |
| TypeScript | `.ts`, `.tsx`                          |
| Rust       | `.rs`                                  |
| Kotlin     | `.kt`, `.kts`                          |
| C++        | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`   |

## When NOT to Use gabb

Fall back to Grep/Read for:
- Non-code files (.json, .md, .yaml, .toml)
- Unsupported languages (.js, .jsx, .go, .java, .c, .h)
- Searching for text content (strings, comments)
