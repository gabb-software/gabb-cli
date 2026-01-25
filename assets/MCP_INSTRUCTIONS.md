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

## `gabb_structure` - File Layout Preview

**After `gabb_symbol` returns a location:** Go directly to `Read` with offset.
Don't call `gabb_structure` on a file where you already know the target line.

**SKIP `gabb_structure` when:**
- `gabb_symbol` already found the exact file:line location
- You're reading a single known file (not choosing between files)
- The file is <200 lines (just read it directly)
- You only need a specific function/class (use offset from symbol search)

**USE `gabb_structure` only when:**
- Multiple files matched and you need to pick the right one
- You need to understand overall file organization before making changes
- The file is very large (>500 lines) AND you don't know which section to read

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
