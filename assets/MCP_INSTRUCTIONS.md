# Gabb MCP Server

Code indexing server providing lightweight file structure previews via SQLite index.

## Task Complexity Assessment

Before using gabb exploration tools, assess whether exploration is needed:

**Go direct (skip exploration) when:**
- Task names a specific file or function (e.g., "fix bug in utils.py")
- Change is localized (e.g., "add parameter to X", "rename Y to Z")
- You can identify the exact target from the task description
- Task is a simple fix with obvious location

**Explore when:**
- Task requires understanding system architecture
- You need to find where something is implemented
- Change affects multiple components or has unclear scope
- You're unfamiliar with the codebase structure

⚠️ **Over-exploration costs time.** A trivial task that could be solved in 15s
can take 60s+ with unnecessary exploration. Match exploration depth to task complexity.

## Anti-Patterns to Avoid

❌ **Over-exploration on obvious targets:**

```
Task: "Fix Http404 handling in technical_404_response in debug views"

BAD (60s): Grep "Http404" → Glob "*debug*" → Read 5 wrong files → finally find it
GOOD (15s): Read django/views/debug.py → Done (task named the file)
```

**Key insight:** If the task names specific files or functions, trust that information. Don't verify what's already stated.

❌ **Task agent for simple lookups:**

```
Task: "Find where UserSerializer is defined"

BAD: Launch Task agent to "explore serializers"
GOOD: Grep "class UserSerializer" → Read the match
```

**Key insight:** Direct tool calls beat exploration agents for specific symbol lookups.

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
