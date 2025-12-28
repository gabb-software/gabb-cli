# gabb_rename

Get all locations to update when renaming a symbol.

```
gabb_rename file="src/payments.py" line=42 character=5 new_name="process_order"
```

Returns edit-ready JSON with exact text spans for each location.

**Parameters:**
- `file`, `line`, `character` - Position of symbol to rename
- `new_name` - The new name
- `limit` - Max locations (default: 100)
