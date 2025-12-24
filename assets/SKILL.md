---
name: gabb-code-navigation
description: |
  Use gabb MCP tools for semantic code navigation. ALWAYS prefer gabb over
  Read/Grep/Glob when exploring code. Gabb understands code semantically -
  it knows what's a function vs a comment, and provides instant results.
---

# Code Navigation with gabb

This project uses gabb for fast, semantic code navigation. Unlike text search,
gabb understands code structure and provides precise file:line:column locations.

## Supported Languages

**gabb indexes these languages and file extensions:**

| Language   | Extensions                                           |
|------------|------------------------------------------------------|
| TypeScript | `.ts`, `.tsx`                                        |
| Rust       | `.rs`                                                |
| Kotlin     | `.kt`, `.kts`                                        |
| C++        | `.cpp`, `.cc`, `.cxx`, `.c++`, `.hpp`, `.hh`, `.hxx`, `.h++` |
| Python     | `.py`, `.pyi`                                        |

**Not indexed** (use Grep/Glob instead):
- JavaScript (`.js`, `.jsx`) — not currently supported
- Plain C (`.c`, `.h`) — not currently supported
- Go, Java, Ruby, and other languages

**Quick decision:** Check the file extension. If it's in the table above, use gabb. Otherwise, use Grep/Glob.

## IMPORTANT: Use gabb Instead of Read/Grep

**Before using Read, Grep, or Glob on code files, STOP and ask:**

1. **Am I looking for a specific symbol?** → `gabb_symbol name="MyType"`
2. **Do I need to understand a file's structure?** → `gabb_structure file="path"`
3. **Am I searching for symbols by pattern?** → `gabb_symbols`
4. **Do I need to trace usage/calls?** → `gabb_usages` / `gabb_callers`
5. **None of the above?** → Fall back to Read/Grep

### Why This Matters

| Situation | Bad Approach | Good Approach |
|-----------|--------------|---------------|
| Need type definition from large file | `Read` entire 2000-line file | `gabb_symbol name="MyType" include_source=true` |
| Understanding unfamiliar file | `Read` then scan for functions | `gabb_structure file="path"` |
| Find where function is defined | `Grep` for function name | `gabb_definition` at call site |
| Find all usages before refactoring | `Grep` for symbol name | `gabb_usages` (semantic, no false matches) |
| Find all structs with "Handler" | `Grep "struct.*Handler"` | `gabb_symbols kind="struct" name_contains="Handler"` |

### Rules of Thumb

- **Use `gabb_structure` before Read** on any file >100 lines
- **Use `gabb_symbol`** when you need a specific struct/type/function definition
- **Use `gabb_symbols`** when searching for symbols by pattern or kind
- **Use `gabb_definition`** when you see a call and want to find where it's defined
- **Use `gabb_usages`** before any refactoring to find all references

### When Read/Grep ARE Appropriate

- Reading non-code files (markdown, config, JSON)
- Searching for literal strings in comments or log messages
- Pattern matching in string content (not symbol names)
- Files gabb doesn't index (non-supported languages)

## Critical Anti-Patterns to AVOID

These patterns waste tokens, slow you down, and ignore gabb's capabilities:

### ❌ Anti-Pattern 1: Find Locations, Then Read

```
gabb_symbols name_contains="config" file="src/main.rs"
→ Returns: mcp_config at line 2624, generate_mcp_config at line 2603
→ Read file_path="src/main.rs" offset=2600 limit=100  ❌ WRONG!
→ Read file_path="src/main.rs" offset=2700 limit=100  ❌ STILL WRONG!
```

**Why it's wrong:** You already asked gabb for the symbols. Just add `include_source=true`.

**Correct:**
```
gabb_symbols name_contains="config" file="src/main.rs" include_source=true
→ Returns: Full source code of both functions in ONE call ✓
```

### ❌ Anti-Pattern 2: Grep for Symbol Definitions

```
Grep pattern="fn mcp_config" path="src/"  ❌
```

**Why it's wrong:** Grep doesn't understand code. It might match comments, strings, or partial names.

**Correct:**
```
gabb_symbol name="mcp_config" include_source=true ✓
```

### ❌ Anti-Pattern 3: Reading Large Files Incrementally

```
Read file_path="src/main.rs" offset=0 limit=200
Read file_path="src/main.rs" offset=200 limit=200
Read file_path="src/main.rs" offset=400 limit=200
... (continues for 3000-line file) ❌
```

**Why it's wrong:** You're reading blindly. Use structure first.

**Correct:**
```
gabb_structure file="src/main.rs"
→ Shows exactly where each function/struct is located
→ Read ONLY the specific 20-50 lines you need ✓
```

### ❌ Anti-Pattern 4: Not Using `include_source=true`

Almost every gabb query benefits from `include_source=true`. If you find yourself
following up a gabb call with Read, you probably forgot this parameter.

| Call | Without include_source | With include_source |
|------|----------------------|---------------------|
| `gabb_symbol name="MyType"` | Location only, need Read | Full source, no Read needed |
| `gabb_symbols kind="function"` | List of locations | List with full implementations |
| `gabb_definition file=... line=...` | Jump location | Jump + see the code |
| `gabb_usages` | Reference locations | References + surrounding code |

**Rule: Default to `include_source=true` unless you explicitly only need locations.**

## Optimal Workflows

### Exploring Unfamiliar Code (The Right Way)

```
Step 1: gabb_structure file="src/large_file.rs"
        → Hierarchical view: 50 functions, 10 structs, exact line ranges

Step 2: Identify the 2-3 symbols you actually care about

Step 3: gabb_symbols name="relevant_fn" include_source=true
        → Get ONLY what you need, with full source

TOTAL: 2 calls. NOT: structure → Read → Read → Read → Read...
```

### Finding and Understanding a Symbol

```
WRONG:
  gabb_symbol name="UserService"  → line 150
  Read file offset=145 limit=50   → get context
  Read file offset=195 limit=50   → get more context

RIGHT:
  gabb_symbol name="UserService" include_source=true context_lines=5
  → ONE call, full source with context, done.
```

### Before Refactoring

```
WRONG:
  Grep pattern="UserService" → 47 matches (includes comments, strings, partials)
  Manually review each...

RIGHT:
  gabb_usages file="src/user.rs" line=10 character=5 format="refactor"
  → Semantic references only, edit-ready JSON
  → Apply with Edit tool
```

## The `include_source=true` Principle

**If you're about to call Read after a gabb tool, STOP.**

Ask yourself: "Could I have gotten the source in my original gabb call?"

The answer is almost always YES:
- `gabb_symbol` → add `include_source=true`
- `gabb_symbols` → add `include_source=true`
- `gabb_definition` → already defaults to `include_source=true`
- `gabb_structure` → add `include_source=true` for specific symbols
- `gabb_usages` → works with include_source
- `gabb_callers/callees` → add `include_source=true`

**Cascade of Reads = Missed opportunity to use gabb properly.**

### After gabb_structure: Surgical Reads Only

`gabb_structure` gives you exact line ranges for every symbol. **Use them.**
The structure output is a replacement for exploratory reading, not a precursor to it.

**Anti-pattern:**
```
gabb_structure file="src/mcp.rs"
→ Shows handle_tools_list at lines 310-839 (15 Tool definitions)
→ Read file_path="src/mcp.rs" offset=310 limit=550  ❌ Reading 550 lines!
```

**Correct pattern:**
```
gabb_structure file="src/mcp.rs"
→ Shows handle_tools_list at lines 310-839, gabb_stats tool at 823-835
→ Read file_path="src/mcp.rs" offset=823 limit=15  ✓ Read ONE example
→ Implement based on the pattern
```

**Rule of thumb:** After getting structure, your total Read should be <100 lines
to understand a pattern. If you're reading 500+ lines after getting structure,
you're ignoring the line numbers it gave you.

**Workflow for adding similar code:**
1. `gabb_structure` — see all existing examples with line ranges
2. Pick ONE simple example, Read just those lines (~20-50 lines)
3. Implement your addition following the pattern
4. Do NOT read "extra context" — the structure already told you everything

## Common Task Patterns

| Task | Use This | Not This |
|------|----------|----------|
| Find a type/struct definition | `gabb_symbol name="MyType"` | Read + grep |
| Understand a large file | `gabb_structure file="path"` | Read entire file |
| Find all structs matching pattern | `gabb_symbols kind="struct" name_contains="X"` | Grep for "struct" |
| See what calls a function | `gabb_callers file="..." line=N character=M` | Grep for function name |
| Find implementations of trait | `gabb_implementations file="..." line=N character=M` | Manual search |
| Get function source code | `gabb_symbol name="fn" include_source=true` | Read file + find function |
| List all functions in directory | `gabb_symbols kind="function" file="src/api/"` | Glob + Read each file |
| Rename a symbol safely | `gabb_rename` + Edit tool | Find/replace (misses some) |

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
| "Rename this function safely" | `gabb_rename` | Returns locations to edit (apply with Edit tool) |
| "What symbols are in this file?" | `gabb_structure` | Hierarchical view with test/prod context |
| "Find all Handler classes" | `gabb_symbols` | Filter by kind, pattern, namespace |
| "Is there duplicate code?" | `gabb_duplicates` | Content-hash based, not text |

**Use grep/ripgrep only when:**
- Searching for literal strings, comments, or log messages
- Pattern matching across non-code files (markdown, config)
- The search term isn't a code symbol (e.g., error messages, URLs)

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
**Get all locations that need editing to rename a symbol.** Does NOT perform the
rename - returns edit-ready JSON that you then apply using the Edit tool.

```
Parameters:
  file: string (required)     - File containing the symbol to rename
  line: integer (required)    - 1-based line number
  character: integer (required) - 1-based column number
  new_name: string (required) - The new name for the symbol
  limit: integer              - Max locations (default: 100)
```

Returns JSON with `old_text`, `new_text`, and exact positions for each location.
Includes both the definition and all usages. Each entry has file, line, column,
end_line, end_column, old_text, new_text, context, and is_definition flag.

Example: Rename `getUserById` to `findUserById`. Point to the function definition
and provide the new name. Get back all 15 locations that need updating, then
apply each edit using the Edit tool with the old_text → new_text replacement.

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
- See "Supported Languages" section above for indexed file types
