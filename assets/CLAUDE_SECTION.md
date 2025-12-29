## Tool Selection: Use gabb for Code Navigation

**This project has gabb MCP tools available. Use them instead of Grep/Read/Glob for code:**

| Task | Use gabb | NOT this |
|------|----------|----------|
| Find function/class | `gabb_symbols name_contains="foo" include_source=true` | `Grep "def foo"` |
| Understand file structure | `gabb_structure file="src/file.rs"` | `Read file.rs` |
| Get symbol definition | `gabb_symbol name="MyType"` | `Read` + search |
| Find usages before refactor | `gabb_usages file="..." line=N character=M` | `Grep "MyType"` |
| Trace call graph | `gabb_callers` / `gabb_callees` | Multiple `Read` calls |

**Key rules:**
- Always add `include_source=true` to get code without needing Read
- Use `gabb_structure` before reading any file >100 lines
- Supported: `.py`, `.rs`, `.ts`, `.tsx`, `.kt`, `.cpp`
- Fall back to Grep/Read only for non-code files or unsupported languages
