# gabb_symbols / gabb_symbol

Find code by name or keyword.

## gabb_symbols - Search by keyword

```
gabb_symbols name_contains="payment" kind="function" include_source=true
```

**Parameters:**
- `name_contains` - Substring match (e.g., "auth" matches "authenticate", "auth_handler")
- `name_pattern` - Glob pattern (e.g., `get*`, `*Handler`, `*_test`)
- `kind` - Filter: `function`, `class`, `method`, `interface`, `type`, `struct`, `enum`, `trait`, `const`, `variable`
- `file` - Filter by path or glob (e.g., `src/**/*.py`)
- `include_source=true` - **Always use this** to get code inline
- `limit` - Max results (default: 50)

## gabb_symbol - Get exact symbol

```
gabb_symbol name="process_payment" include_source=true
```

Use when you know the exact name.
