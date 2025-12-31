## Tool Selection: Use gabb for Code Navigation

This project has gabb MCP tools available. Use them instead of Grep/Read/Glob for code.

| Task | Use gabb | NOT this |
|------|----------|----------|
| Find function/class | `gabb_symbols` | `Grep "def foo"` |
| Understand file structure | `gabb_structure` | `Read file.rs` |
| Find usages | `gabb_usages` | `Grep "MyType"` |

Fall back to Grep/Read for non-code files or unsupported languages.
