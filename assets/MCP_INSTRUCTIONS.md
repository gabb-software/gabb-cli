# Gabb MCP Server Instructions

You have access to gabb code navigation tools. These tools provide precise, indexed access to code symbols and are significantly faster and more accurate than text-based search.

## REQUIRED: Use Gabb for Code Navigation

When searching for code symbols in supported languages (Python, TypeScript, Rust, Kotlin, C++), you MUST use gabb tools instead of Grep/Glob/Read:

| Task | MUST Use | NOT This |
|------|----------|----------|
| Find function/class definition | `gabb_symbols` or `gabb_symbol` | `Grep "def foo"` |
| Understand file structure | `gabb_structure` | `Read` entire file |
| Find all usages before refactoring | `gabb_usages` | `Grep "function_name"` |
| Jump to where symbol is defined | `gabb_definition` | Manual searching |
| Trace what calls a function | `gabb_callers` | Multiple Grep calls |
| Trace what a function calls | `gabb_callees` | Reading and following code |
| Find interface implementations | `gabb_implementations` | Grep for class names |

## Why This Matters

- **gabb_symbols** returns exact symbol locations with optional source code
- **Grep** returns text matches including comments, strings, and false positives
- **gabb tools are indexed** - O(1) lookup vs O(n) file scanning

## Workflow

1. **First**: Use `gabb_structure` to get a cheap overview of any file >100 lines
2. **Then**: Use `gabb_symbol name="X"` to find the exact location
3. **If you need source**: Use `Read` with offset/limit to read just those lines
4. **Before refactoring**: Use `gabb_usages` to find all references

**Token efficiency**: Prefer `gabb_structure` + targeted `Read` over `include_source=true`. Only use `include_source=true` when you need source for multiple symbols at once.

## When to Fall Back to Grep/Read

Only use Grep/Read for:
- Unsupported languages (.js, .go, .java, .json, .md, .yaml)
- Searching non-code content (log messages, comments, config values)
- Text that isn't a code symbol
