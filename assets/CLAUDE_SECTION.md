## Important: Tool Selection: Use gabb for Code Navigation

**This project has gabb MCP tools available for PRECISE code lookups:**

| Task | Use gabb | NOT this |
|------|----------|----------|
| Preview file structure (cheap) | `gabb_structure file="src/file.rs"` | `Read file.rs` |
| Get specific symbol by name | `gabb_symbol name="MyClass" include_source=true` | `Read` + search |
| Find usages before refactor | `gabb_usages file="..." line=N character=M` | `Grep "MyType"` |
| Trace call graph | `gabb_callers` / `gabb_callees` | Multiple `Read` calls |

**Key rules:**
- `gabb_structure` is CHEAP - use it first to see file contents without source code
- Use `include_source=true` ONLY for specific symbol lookups (exact name), not broad searches
- For broad exploration, still use Task/Explore agent - gabb is for precision, not discovery
- Supported: `.py`, `.rs`, `.ts`, `.tsx`, `.kt`, `.cpp`
- Fall back to Grep/Read for non-code files or unsupported languages
