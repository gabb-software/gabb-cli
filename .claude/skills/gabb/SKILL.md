---
name: gabb-code-navigation
description: |
  Use gabb MCP tools for semantic code navigation. Prefer gabb over grep/ripgrep
  when navigating code structure: finding definitions, usages, implementations.
  Gabb understands code semantically - it knows what's a function vs a comment.
---

# Code Navigation with gabb

This project uses gabb for fast, semantic code navigation. Unlike text search,
gabb understands code structure and provides precise file:line:column locations.

## When to Use gabb vs grep/ripgrep

**Use gabb when you need to understand code relationships:**

| Task | gabb Tool | Why Better Than grep |
|------|-----------|---------------------|
| "Where is this function defined?" | `gabb_definition` | Follows imports, finds actual definition |
| "What calls this function?" | `gabb_usages` | Semantic refs only, no false matches |
| "What implements this interface?" | `gabb_implementations` | Understands type relationships |
| "What does this class inherit from?" | `gabb_supertypes` | Navigates extends/implements edges |
| "What inherits from this?" | `gabb_subtypes` | Finds all derived types |
| "What calls this function?" | `gabb_callers` | Call graph: trace backwards through callers |
| "What does this function call?" | `gabb_callees` | Call graph: trace forwards through callees |
| "Rename this function safely" | `gabb_rename` | Returns edit-ready JSON for all locations |
| "What symbols are in this file?" | `gabb_structure` | Hierarchical view with test/prod context |
| "Find all Handler classes" | `gabb_symbols` | Filter by kind, pattern, namespace |
| "Is there duplicate code?" | `gabb_duplicates` | Content-hash based, not text |

**Use grep/ripgrep when:**
- Searching for literal strings, comments, or log messages
- Pattern matching across non-code files
- The search term isn't a code symbol

## Position-Based Navigation (Key Concept)

The most powerful gabb tools use **position-based lookup**: you point to a location
in the code (file:line:column), and gabb tells you about the symbol there.

```
You're reading code at src/auth.rs:45:10 and see `validate_token(...)`
  ↓
gabb_definition file="src/auth.rs" line=45 character=10
  ↓
Returns: Definition at src/tokens.rs:23:1 with source code
```

This matches how you actually read code: "What is this thing I'm looking at?"

## MCP Tools Reference

### gabb_definition
**Jump from usage to definition.** The most common navigation operation.

```
Parameters:
  file: string (required)     - File containing the symbol usage
  line: integer (required)    - 1-based line number
  character: integer (required) - 1-based column number
  include_source: boolean     - Include definition source (default: true)
  context_lines: integer      - Lines of context around symbol
```

Example: You see `processOrder(...)` at line 50. Call gabb_definition to find
where `processOrder` is defined, even if it's in another file or imported.

### gabb_usages
**Find all references to a symbol.** Essential before refactoring.

```
Parameters:
  file: string (required)     - File containing the symbol
  line: integer (required)    - 1-based line number
  character: integer (required) - 1-based column number
  limit: integer              - Max results (default: 50)
  format: string              - "default" or "refactor" (for rename operations)
  include_definition: bool    - Include definition in refactor output (default: true)
```

Example: Before renaming `UserService`, point to its definition and find
everywhere it's used. Unlike grep, won't match "UserService" in comments.

Use `format: "refactor"` for rename operations - returns JSON with exact spans
and `old_text` for each location, ready for Edit tool.

### gabb_implementations
**Find all types implementing an interface/trait.**

```
Parameters:
  file: string (required)     - File containing the interface/trait
  line: integer (required)    - 1-based line number
  character: integer (required) - 1-based column number
  limit: integer              - Max results (default: 50)
```

Example: You see `trait Handler`. Find all structs that `impl Handler`.

### gabb_supertypes
**Find parent types (superclasses, implemented interfaces/traits).**

```
Parameters:
  file: string (required)     - File containing the type
  line: integer (required)    - 1-based line number
  character: integer (required) - 1-based column number
  transitive: bool            - Include full hierarchy chain (default: false)
  include_source: bool        - Include parent source code
  limit: integer              - Max results (default: 50)
```

Example: You see `class AdminUser`. Find what it extends and implements.
Use `transitive: true` to see the full inheritance chain.

### gabb_subtypes
**Find child types (subclasses, implementors).**

```
Parameters:
  file: string (required)     - File containing the type/interface/trait
  line: integer (required)    - 1-based line number
  character: integer (required) - 1-based column number
  transitive: bool            - Include full hierarchy chain (default: false)
  include_source: bool        - Include child source code
  limit: integer              - Max results (default: 50)
```

Example: You see `class BaseService`. Find all classes that extend it.
Essential for impact analysis when modifying base classes.

### gabb_callers
**Find all functions that call a given function.** Trace execution backwards.

```
Parameters:
  file: string (required)     - File containing the function
  line: integer (required)    - 1-based line number
  character: integer (required) - 1-based column number
  transitive: bool            - Include full call chain (default: false)
  include_source: bool        - Include caller source code
  limit: integer              - Max results (default: 50)
```

Example: You see `validate_token`. Find all functions that call it.
Use `transitive: true` to see the full call chain (callers of callers).

### gabb_callees
**Find all functions called by a given function.** Trace execution forwards.

```
Parameters:
  file: string (required)     - File containing the function
  line: integer (required)    - 1-based line number
  character: integer (required) - 1-based column number
  transitive: bool            - Include full call chain (default: false)
  include_source: bool        - Include callee source code
  limit: integer              - Max results (default: 50)
```

Example: You see `process_request`. Find all functions it calls.
Use `transitive: true` to see the full call chain (callees of callees).

### gabb_rename
**Get all locations to update when renaming a symbol.** The safest way to rename.

```
Parameters:
  file: string (required)     - File containing the symbol to rename
  line: integer (required)    - 1-based line number
  character: integer (required) - 1-based column number
  new_name: string (required) - The new name for the symbol
  limit: integer              - Max locations (default: 100)
```

Returns JSON with `old_text`, `new_text`, and exact positions for each location.
Includes both the definition and all usages. Output is structured for direct
use with Edit tool - each entry has file, line, column, old_text, new_text.

Example: Rename `getUserById` to `findUserById`. Point to the function definition
and provide the new name. Get back all 15 locations that need updating.

### gabb_symbols
**Search for symbols by name, pattern, or kind.** For exploration.

```
Parameters:
  name: string           - Exact name match
  name_pattern: string   - Glob pattern: 'get*', '*Handler', '*User*'
  name_contains: string  - Substring: 'User' matches 'getUser', 'UserService'
  name_fts: string       - Fuzzy search: 'usrsvc' matches 'UserService'
  case_insensitive: bool - Case-insensitive matching
  kind: string           - function, class, interface, struct, enum, trait, method
  file: string           - Filter by path, directory ('src/'), or glob ('**/*.ts')
  namespace: string      - Filter by qualifier: 'std::collections', 'myapp::*'
  scope: string          - Filter by container: 'MyClass' for its methods
  include_source: bool   - Include source code
  context_lines: integer - Lines of context (with include_source)
  limit: integer         - Max results (default: 50)
  offset: integer        - Skip N results (pagination)
  after: string          - Cursor-based pagination (symbol ID)
```

### gabb_symbol
**Get details for a known symbol name.**

```
Parameters:
  name: string (required) - Exact symbol name
  kind: string            - Disambiguate if multiple symbols share the name
  include_source: bool    - Include source code
```

### gabb_structure
**View file structure with all symbols hierarchically.**

```
Parameters:
  file: string (required) - File to analyze
  include_source: bool    - Include source snippets
  context_lines: integer  - Lines of context
```

Returns symbols nested by containment (methods inside classes), with:
- Start/end positions
- `[test]` or `[prod]` context indicator
- Visibility (pub, private, etc.)

### gabb_duplicates
**Find copy-paste code for refactoring.**

```
Parameters:
  kind: string      - Filter by symbol kind
  min_count: integer - Minimum duplicates (default: 2)
  limit: integer     - Max duplicate groups (default: 20)
```

### gabb_includers
**Find files that #include a header (C/C++).**

```
Parameters:
  file: string (required) - Header file path
  transitive: bool        - Include indirect includers
  limit: integer          - Max results (default: 50)
```

### gabb_includes
**Find headers included by a file (C/C++).**

```
Parameters:
  file: string (required) - Source file path
  transitive: bool        - Follow include chains
  limit: integer          - Max results (default: 50)
```

### gabb_daemon_status
**Check if indexing daemon is running.**

No parameters. Returns daemon PID, version, indexed file count.

### gabb_stats
**Get comprehensive index statistics.**

No parameters. Returns:
- File counts by language (typescript, rust, kotlin, etc.)
- Symbol counts by kind (function, class, interface, etc.)
- Index size in bytes and last update time
- Schema version

Use to understand the scope of the indexed codebase or verify indexing is complete.

## Common Workflows

### Understanding unfamiliar code
1. `gabb_structure` - See what's in the file
2. `gabb_definition` - Follow calls to understand flow
3. `gabb_usages` - See how functions are used elsewhere

### Safe refactoring
1. `gabb_usages` - Find ALL references before changing
2. `gabb_implementations` - Check implementing types
3. `gabb_subtypes` - Check what inherits from the type (impact analysis)
4. Make changes with confidence

### Understanding type hierarchies
1. `gabb_supertypes` - See what a class inherits from
2. `gabb_subtypes` - See what inherits from a class/interface
3. Use `transitive: true` to see the full hierarchy

### Tracing execution flow
1. `gabb_callers` - Find who calls a function (trace backwards)
2. `gabb_callees` - Find what a function calls (trace forwards)
3. Use `transitive: true` to see the full call chain

### Finding code patterns
1. `gabb_symbols kind="function" name_pattern="*Handler"` - Find handlers
2. `gabb_duplicates kind="function"` - Find copy-paste to consolidate

## Tips

- The daemon auto-starts when needed - no manual setup required
- Results include precise `file:line:column` locations
- Index updates automatically when files change
- Supports TypeScript, Rust, Kotlin, and C++
