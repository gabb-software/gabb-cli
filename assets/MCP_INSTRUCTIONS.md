# Gabb MCP Server

Code indexing server providing semantic symbol search and file structure previews via SQLite index.

## Search Strategy Decision Flow

When you need to find code, follow this order:

1. **Task names specific file/function?** → Read directly (skip exploration)
2. **Looking for a code construct by name?** → `gabb_symbol`
3. **Looking for text content (strings, error messages)?** → Grep
4. **Need to understand file layout?** → `gabb_structure`

## `gabb_symbol` - Workspace Symbol Search

Search for symbols (functions, classes, methods) by name across the workspace.

**When to use:**
- Task mentions a function/class/method name to find or fix
- You need to find where something is defined
- Grep would return too many false positives for a common term

**Example:**
```
Task: "The update_proxy_model_permissions function has a bug"

gabb_symbol name="update_proxy_model_permissions"
→ function update_proxy_model_permissions [prod] django/contrib/auth/migrations/0011_update_proxy_permissions.py:5:1
```

**Anti-pattern:**
```
Task: "Fix the update_proxy_model_permissions function"

BAD (75s): Grep "IntegrityError" → 48 files → spawn Task agent → still confused
GOOD (5s): gabb_symbol name="update_proxy_model_permissions" → 1 match → done
```

**Use Grep instead when:**
- Searching for error messages or string literals
- Looking for text patterns, not code identifiers

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

## When to Skip Exploration Entirely

Go DIRECTLY to the file (no gabb_symbol, no Grep, no gabb_structure) when ANY of these are true:

1. **Task names a specific file path**: "fix bug in django/db/models/fields.py"
   → Read that file directly

2. **Hint/diff shows exact location**: The task includes a code snippet or diff
   → Read the file at that location

3. **Single-line fix**: "change X to Y", "rename A to B", "fix typo"
   → Find and read the obvious file

4. **Error traceback provided**: Stack trace points to specific file:line
   → Read that file at that line

**The test:** If you can name the target file from the task description alone,
skip exploration and just Read it.

## `gabb_structure` - File Layout Preview

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
| Go         | `.go`                                  |
| Kotlin     | `.kt`, `.kts`                          |
| C++        | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`   |

## When NOT to Use gabb

Fall back to Grep/Read for:
- Non-code files (.json, .md, .yaml, .toml)
- Unsupported languages (.js, .jsx, .java, .c, .h)
- Searching for text content (strings, comments)
