# Design: `gabb install-global` / `gabb uninstall-global`

**Date:** 2026-04-04
**Status:** Draft
**Goal:** Allow users to install gabb's MCP config and skill file to `~/.claude/` (user-scoped) instead of the project-local `.claude/`, so gabb is available in every Claude Code session without per-project setup.

## CLI Surface

Two new top-level subcommands:

```
gabb install-global [--mcp] [--skill]
gabb uninstall-global [--mcp] [--skill]
```

- **No flags** = install/uninstall both MCP config and skill file (the common case)
- **`--mcp`** = only MCP config
- **`--skill`** = only skill file
- No `--gitignore` flag (irrelevant at global scope)

## Target Paths

| Asset | Global path |
|-------|------------|
| MCP config | `~/.claude/mcp.json` |
| Skill file | `~/.claude/skills/gabb/SKILL.md` |

## MCP Config: Global vs Local

### Local (existing `gabb init --mcp`)

```json
{
  "mcpServers": {
    "gabb": {
      "command": "/absolute/path/to/gabb",
      "args": ["mcp-server", "--workspace", "."]
    }
  }
}
```

Uses `--workspace .` (relative path, git-friendly).

### Global (new `gabb install-global --mcp`)

```json
{
  "mcpServers": {
    "gabb": {
      "command": "/absolute/path/to/gabb",
      "args": ["mcp-server"]
    }
  }
}
```

Omits `--workspace` entirely. The MCP server infers the workspace from the working directory provided by Claude Code at runtime. This means gabb works in whichever project Claude Code is open in.

## Skill File

Same `assets/SKILL.md` content as local installs. No content changes needed. Claude Code discovers skills from `~/.claude/skills/` identically to `.claude/skills/`.

## Code Reuse Strategy

The existing codebase has the right abstractions. The new commands are thin wrappers over shared helpers.

### Refactored helpers

1. **`init_mcp_config(root)` -> `install_mcp_config(target_dir, include_workspace)`**
   - `target_dir`: the directory containing `mcp.json` (e.g., `./.claude/` or `~/.claude/`)
   - `include_workspace`: whether to add `--workspace .` to the args
   - Called by `gabb init --mcp` with `(project_root/.claude, true)` and by `install-global` with `(~/.claude, false)`

2. **`init_skill(root)` -> `install_skill(target_dir)`**
   - `target_dir`: the base `.claude/` directory
   - Writes to `{target_dir}/skills/gabb/SKILL.md`
   - Called by `gabb init --skill` with `project_root/.claude` and by `install-global` with `~/.claude`

3. **`install_to_config_file(config_path, root, use_absolute)`** in `mcp_config.rs`
   - Already accepts arbitrary config paths. `install-global` calls this directly with `~/.claude/mcp.json`.

4. **New helper: `global_claude_dir() -> PathBuf`**
   - Returns `~/.claude/` using `dirs::home_dir()`
   - Used by both `install-global` and `uninstall-global`

5. **`generate_mcp_config(root, use_absolute)` needs extension**
   - Add a parameter or variant that omits `--workspace` for global installs
   - Use an enum `WorkspaceMode { Relative, Absolute, Omit }` to replace the current `use_absolute: bool` parameter, keeping all three behaviors explicit

### New module

`src/commands/install_global.rs` — contains `install_global()` and `uninstall_global()` functions that wire up the target path and call the shared helpers above.

## Uninstall Logic

`gabb uninstall-global`:

- **`--mcp`**: Remove the `"gabb"` key from `~/.claude/mcp.json`. Does not delete the file (other servers may be configured). If gabb is the only entry, removes the file.
- **`--skill`**: Delete the `~/.claude/skills/gabb/` directory.
- **No flags**: Both of the above.

## Error Handling

All behaviors match existing patterns in `init` and `mcp install`:

- `~/.claude/` doesn't exist: create it
- `mcp.json` already has gabb: print "already configured", skip
- Skill file exists and is identical: skip
- Skill file exists and differs: update it
- Backup `mcp.json` before modification (`.json.bak`)
- Home directory not found: error with clear message

## Interaction with Existing Commands

No changes to:

- `gabb init` (local installs unchanged)
- `gabb mcp install` (Desktop/Code scopes unchanged)
- `gabb mcp uninstall` (existing scopes unchanged)
- Daemon, indexer, MCP server, or query commands

## Testing

- Unit tests for refactored helpers with global path arguments
- Integration test for `install-global` writing to a temp dir (mock `~/.claude/`)
- Integration test for `uninstall-global` cleaning up correctly
- Test that global MCP config omits `--workspace`
- Test that local MCP config still includes `--workspace .` (no regression)
- Test merge behavior: global `mcp.json` with existing servers preserves them

## Documentation Updates Required

Per CLAUDE.md checklist:

- **README.md**: Add `install-global` / `uninstall-global` to command reference
- **CLAUDE.md**: Add to "Query Commands" section
- **`assets/SKILL.md`**: Mention global install option
- **`.claude/skills/gabb/SKILL.md`**: Update if it exists locally
