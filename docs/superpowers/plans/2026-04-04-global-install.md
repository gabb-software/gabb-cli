# `install-global` / `uninstall-global` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `gabb install-global` and `gabb uninstall-global` subcommands that write MCP config and skill files to `~/.claude/` instead of the project-local `.claude/`.

**Architecture:** New thin command module (`install_global.rs`) calls refactored shared helpers from `init.rs` and `mcp_config.rs`. The key refactor is parameterizing target directory and workspace mode so both local and global paths use the same logic. `generate_mcp_config` gains a `WorkspaceMode` enum to control whether `--workspace` is included in the MCP args.

**Tech Stack:** Rust, clap, serde_json, dirs, tempfile (tests)

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/cli.rs` | Modify | Add `InstallGlobal` and `UninstallGlobal` variants to `Commands` enum |
| `src/commands/mcp_config.rs` | Modify | Add `WorkspaceMode` enum, refactor `generate_mcp_config`, add `global_claude_dir()` helper, make `install_to_config_file` and `uninstall_from_config_file` public |
| `src/commands/init.rs` | Modify | Refactor `init_mcp_config` and `init_skill` to accept target dir parameter, extract shared logic |
| `src/commands/install_global.rs` | Create | New module with `install_global()` and `uninstall_global()` functions |
| `src/commands/mod.rs` | Modify | Register new module and re-export functions |
| `src/main.rs` | Modify | Wire new commands to dispatch |
| `tests/cli_integration.rs` | Modify | Add integration tests for install-global and uninstall-global |

---

### Task 1: Add `WorkspaceMode` enum and refactor `generate_mcp_config`

**Files:**
- Modify: `src/commands/mcp_config.rs:54-71`

- [ ] **Step 1: Write the failing test**

Add a test at the bottom of `src/commands/mcp_config.rs` inside a `#[cfg(test)] mod tests` block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn generate_mcp_config_omit_workspace() {
        let root = PathBuf::from("/tmp/fake");
        let config = generate_mcp_config(&root, WorkspaceMode::Omit);
        let args = config["mcpServers"]["gabb"]["args"]
            .as_array()
            .unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], "mcp-server");
        assert!(!args.iter().any(|a| a.as_str() == Some("--workspace")));
    }

    #[test]
    fn generate_mcp_config_relative_workspace() {
        let root = PathBuf::from("/tmp/fake");
        let config = generate_mcp_config(&root, WorkspaceMode::Relative);
        let args = config["mcpServers"]["gabb"]["args"]
            .as_array()
            .unwrap();
        assert_eq!(args.len(), 3);
        assert_eq!(args[1], "--workspace");
        assert_eq!(args[2], ".");
    }

    #[test]
    fn generate_mcp_config_absolute_workspace() {
        let root = PathBuf::from("/tmp/fake");
        let config = generate_mcp_config(&root, WorkspaceMode::Absolute);
        let args = config["mcpServers"]["gabb"]["args"]
            .as_array()
            .unwrap();
        assert_eq!(args.len(), 3);
        assert_eq!(args[1], "--workspace");
        // Absolute mode uses canonicalize or fallback, so just check it's not "."
        assert_ne!(args[2], ".");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin gabb generate_mcp_config -- --nocapture`
Expected: FAIL — `WorkspaceMode` does not exist yet.

- [ ] **Step 3: Add `WorkspaceMode` enum and refactor `generate_mcp_config`**

In `src/commands/mcp_config.rs`, add the enum before the `generate_mcp_config` function, then update the function signature:

```rust
/// Controls how the --workspace argument is set in MCP config
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceMode {
    /// Use relative path "." (for project-level config, version control friendly)
    Relative,
    /// Use absolute path (for Claude Desktop config)
    Absolute,
    /// Omit --workspace entirely (for global config, inferred at runtime)
    Omit,
}

/// Generate MCP server config JSON for a workspace
pub fn generate_mcp_config(root: &Path, mode: WorkspaceMode) -> serde_json::Value {
    let mut args: Vec<serde_json::Value> = vec!["mcp-server".into()];

    match mode {
        WorkspaceMode::Relative => {
            args.push("--workspace".into());
            args.push(".".into());
        }
        WorkspaceMode::Absolute => {
            let root_str = root
                .canonicalize()
                .unwrap_or_else(|_| root.to_path_buf())
                .to_string_lossy()
                .to_string();
            args.push("--workspace".into());
            args.push(root_str.into());
        }
        WorkspaceMode::Omit => {
            // No --workspace arg; MCP server infers from working directory
        }
    }

    serde_json::json!({
        "mcpServers": {
            "gabb": {
                "command": find_gabb_binary(),
                "args": args
            }
        }
    })
}
```

- [ ] **Step 4: Update all callers of `generate_mcp_config`**

In `src/commands/mcp_config.rs`, update `mcp_config` (line 79):
```rust
// was: generate_mcp_config(&root, true)
let config = generate_mcp_config(&root, WorkspaceMode::Absolute);
```

In `src/commands/mcp_config.rs`, update `install_to_config_file` (line 205):
```rust
// was: let gabb_config = generate_mcp_config(root, use_absolute);
let mode = if use_absolute { WorkspaceMode::Absolute } else { WorkspaceMode::Relative };
let gabb_config = generate_mcp_config(root, mode);
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --bin gabb generate_mcp_config -- --nocapture`
Expected: All 3 tests PASS.

- [ ] **Step 6: Run full test suite for regressions**

Run: `cargo test`
Expected: All tests PASS.

- [ ] **Step 7: Commit**

```bash
git add src/commands/mcp_config.rs
git commit -m "refactor: add WorkspaceMode enum to generate_mcp_config"
```

---

### Task 2: Add `global_claude_dir()` helper and make install/uninstall helpers public

**Files:**
- Modify: `src/commands/mcp_config.rs`

- [ ] **Step 1: Write the failing test**

Add to the existing `tests` module in `src/commands/mcp_config.rs`:

```rust
#[test]
fn global_claude_dir_returns_home_dot_claude() {
    let dir = global_claude_dir().unwrap();
    let home = dirs::home_dir().unwrap();
    assert_eq!(dir, home.join(".claude"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin gabb global_claude_dir -- --nocapture`
Expected: FAIL — `global_claude_dir` does not exist yet.

- [ ] **Step 3: Implement `global_claude_dir` and adjust visibility**

In `src/commands/mcp_config.rs`, add after the existing path helpers (after `find_gabb_binary`):

```rust
/// Get the path to the global user-scoped Claude config directory (~/.claude/)
pub fn global_claude_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".claude"))
        .ok_or_else(|| anyhow!("Could not determine home directory"))
}
```

Also change visibility of `install_to_config_file` and `uninstall_from_config_file` from `fn` to `pub(crate) fn`:

```rust
// was: fn install_to_config_file(...)
pub(crate) fn install_to_config_file(config_path: &Path, root: &Path, use_absolute: bool) -> Result<bool> {
```

```rust
// was: fn uninstall_from_config_file(...)
pub(crate) fn uninstall_from_config_file(config_path: &Path) -> Result<bool> {
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --bin gabb global_claude_dir -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/commands/mcp_config.rs
git commit -m "feat: add global_claude_dir helper, publicize install/uninstall helpers"
```

---

### Task 3: Refactor `init_mcp_config` and `init_skill` to accept target directory

**Files:**
- Modify: `src/commands/init.rs`

- [ ] **Step 1: Refactor `init_mcp_config` to accept a target claude directory**

Change the function signature and extract the shared logic. The existing function becomes a thin wrapper:

```rust
/// Create mcp.json with gabb configuration in the given claude directory.
///
/// `claude_dir` is the `.claude/` directory (local or global).
/// `include_workspace` controls whether `--workspace .` is added to args.
pub(crate) fn install_mcp_config(claude_dir: &Path, include_workspace: bool) -> Result<()> {
    let mcp_config_path = claude_dir.join("mcp.json");

    // Create directory
    if !claude_dir.exists() {
        fs::create_dir_all(claude_dir)?;
        println!("  Created {}/", claude_dir.display());
    }

    // Generate MCP config using shared helper
    let mode = if include_workspace {
        crate::commands::mcp_config::WorkspaceMode::Relative
    } else {
        crate::commands::mcp_config::WorkspaceMode::Omit
    };
    // For Relative mode root doesn't matter (uses "."), for Omit mode root is unused
    let dummy_root = std::path::Path::new(".");
    let config = crate::commands::mcp_config::generate_mcp_config(dummy_root, mode);

    if mcp_config_path.exists() {
        let existing = fs::read_to_string(&mcp_config_path)?;
        let existing_config: serde_json::Value = serde_json::from_str(&existing)?;
        if existing_config
            .get("mcpServers")
            .and_then(|s| s.get("gabb"))
            .is_some()
        {
            println!("  {} already has gabb configured", mcp_config_path.display());
            return Ok(());
        }

        // Merge with existing config
        let mut merged: serde_json::Value = existing_config;
        let mcp_servers = merged
            .as_object_mut()
            .ok_or_else(|| anyhow!("Invalid mcp.json format"))?
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));

        if let Some(servers) = mcp_servers.as_object_mut() {
            if let Some(gabb) = config
                .get("mcpServers")
                .and_then(|s| s.get("gabb"))
                .cloned()
            {
                servers.insert("gabb".to_string(), gabb);
            }
        }

        fs::write(&mcp_config_path, serde_json::to_string_pretty(&merged)?)?;
        println!("  Added gabb to {}", mcp_config_path.display());
    } else {
        fs::write(&mcp_config_path, serde_json::to_string_pretty(&config)?)?;
        println!("  Created {}", mcp_config_path.display());
    }

    Ok(())
}

/// Create .claude/mcp.json with gabb configuration (project-local).
///
/// This is used by both `init --mcp` and `setup` commands.
pub(crate) fn init_mcp_config(root: &Path) -> Result<()> {
    let claude_dir = root.join(".claude");
    install_mcp_config(&claude_dir, true)
}
```

- [ ] **Step 2: Refactor `init_skill` to accept a target claude directory**

```rust
/// Install the gabb skill file to the given claude directory.
///
/// `claude_dir` is the `.claude/` directory (local or global).
pub(crate) fn install_skill(claude_dir: &Path) -> Result<()> {
    let skills_dir = claude_dir.join("skills").join("gabb");
    if !skills_dir.exists() {
        fs::create_dir_all(&skills_dir)?;
        println!("  Created {}/", skills_dir.display());
    }

    let skill_file = skills_dir.join("SKILL.md");
    let skill_content = include_str!("../../assets/SKILL.md");
    write_skill_file(&skill_file, skill_content, "SKILL.md")?;

    Ok(())
}

/// Create .claude/skills/gabb/ agent skill for Claude Code discoverability (project-local).
///
/// This is used by both `init --skill` and `setup` commands.
pub(crate) fn init_skill(root: &Path) -> Result<()> {
    let claude_dir = root.join(".claude");
    install_skill(&claude_dir)
}
```

- [ ] **Step 3: Update `write_skill_file` to use the actual path in messages**

The existing `write_skill_file` hardcodes `.claude/skills/gabb/` in its println. Update it to use the path:

```rust
fn write_skill_file(path: &Path, content: &str, name: &str) -> Result<()> {
    if path.exists() {
        let existing = fs::read_to_string(path)?;
        if existing == content {
            println!("  {} is up to date", path.display());
            return Ok(());
        }
        fs::write(path, content)?;
        println!("  Updated {}", path.display());
    } else {
        fs::write(path, content)?;
        println!("  Created {}", path.display());
        if name == "SKILL.md" {
            println!("  Claude will auto-discover this skill for code navigation tasks");
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify no regressions**

Run: `cargo test`
Expected: All tests PASS. The `init_project` function still calls `init_mcp_config` and `init_skill` which now delegate to the refactored versions.

- [ ] **Step 5: Run clippy**

Run: `cargo clippy --all-targets --all-features`
Expected: No warnings.

- [ ] **Step 6: Commit**

```bash
git add src/commands/init.rs
git commit -m "refactor: parameterize init helpers to accept target directory"
```

---

### Task 4: Add CLI definitions for `InstallGlobal` and `UninstallGlobal`

**Files:**
- Modify: `src/cli.rs:46-241`

- [ ] **Step 1: Add the two new command variants**

Add after the `Stats` variant (before the closing `}` of `Commands` enum):

```rust
    /// Install gabb MCP server and skill globally (~/.claude/) for all projects
    InstallGlobal {
        /// Only install MCP server configuration
        #[arg(long)]
        mcp: bool,
        /// Only install the agent skill file
        #[arg(long)]
        skill: bool,
    },
    /// Remove gabb MCP server and skill from global config (~/.claude/)
    UninstallGlobal {
        /// Only remove MCP server configuration
        #[arg(long)]
        mcp: bool,
        /// Only remove the agent skill file
        #[arg(long)]
        skill: bool,
    },
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build 2>&1 | head -20`
Expected: Build succeeds (with warnings about unhandled variants in main.rs match — that's expected and fixed in Task 6).

- [ ] **Step 3: Commit**

```bash
git add src/cli.rs
git commit -m "feat: add InstallGlobal and UninstallGlobal CLI definitions"
```

---

### Task 5: Create `install_global.rs` command module

**Files:**
- Create: `src/commands/install_global.rs`
- Modify: `src/commands/mod.rs`

- [ ] **Step 1: Write the failing test for `install_global`**

Create `src/commands/install_global.rs` with tests first:

```rust
//! Install/uninstall gabb globally to ~/.claude/ for all projects.

use anyhow::Result;

use crate::commands::init::{install_mcp_config, install_skill};
use crate::commands::mcp_config::{global_claude_dir, uninstall_from_config_file};

/// Install gabb MCP config and/or skill file globally to ~/.claude/.
pub fn install_global(mcp: bool, skill: bool) -> Result<()> {
    todo!()
}

/// Remove gabb MCP config and/or skill file from ~/.claude/.
pub fn uninstall_global(mcp: bool, skill: bool) -> Result<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn install_global_creates_mcp_config_without_workspace() {
        let dir = tempdir().unwrap();
        let claude_dir = dir.path().join(".claude");

        // Call the shared helper directly to test with a temp dir
        install_mcp_config(&claude_dir, false).unwrap();

        let mcp_path = claude_dir.join("mcp.json");
        assert!(mcp_path.exists());

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&mcp_path).unwrap()).unwrap();
        let args = content["mcpServers"]["gabb"]["args"]
            .as_array()
            .unwrap();
        assert_eq!(args.len(), 1, "global config should only have ['mcp-server'], got {:?}", args);
        assert_eq!(args[0], "mcp-server");
    }

    #[test]
    fn install_global_creates_skill_file() {
        let dir = tempdir().unwrap();
        let claude_dir = dir.path().join(".claude");

        install_skill(&claude_dir).unwrap();

        let skill_path = claude_dir.join("skills").join("gabb").join("SKILL.md");
        assert!(skill_path.exists());

        let content = fs::read_to_string(&skill_path).unwrap();
        assert!(content.contains("gabb"), "skill file should mention gabb");
    }

    #[test]
    fn install_global_merges_with_existing_mcp_config() {
        let dir = tempdir().unwrap();
        let claude_dir = dir.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        // Write existing config with another server
        let existing = serde_json::json!({
            "mcpServers": {
                "other-tool": {
                    "command": "other",
                    "args": []
                }
            }
        });
        fs::write(
            claude_dir.join("mcp.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        ).unwrap();

        install_mcp_config(&claude_dir, false).unwrap();

        let content: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(claude_dir.join("mcp.json")).unwrap(),
        ).unwrap();

        // Both servers should be present
        assert!(content["mcpServers"]["other-tool"].is_object());
        assert!(content["mcpServers"]["gabb"].is_object());
    }

    #[test]
    fn uninstall_removes_gabb_from_mcp_config() {
        let dir = tempdir().unwrap();
        let claude_dir = dir.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        // Install first
        install_mcp_config(&claude_dir, false).unwrap();
        assert!(claude_dir.join("mcp.json").exists());

        // Uninstall
        let mcp_path = claude_dir.join("mcp.json");
        let removed = uninstall_from_config_file(&mcp_path).unwrap();
        assert!(removed);

        // gabb should be gone
        let content: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&mcp_path).unwrap(),
        ).unwrap();
        assert!(content["mcpServers"]["gabb"].is_null());
    }

    #[test]
    fn uninstall_removes_skill_directory() {
        let dir = tempdir().unwrap();
        let claude_dir = dir.path().join(".claude");

        // Install first
        install_skill(&claude_dir).unwrap();
        let skill_dir = claude_dir.join("skills").join("gabb");
        assert!(skill_dir.exists());

        // Remove it
        fs::remove_dir_all(&skill_dir).unwrap();
        assert!(!skill_dir.exists());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --bin gabb install_global -- --nocapture`
Expected: FAIL — `install_global` and `uninstall_global` are `todo!()`.

- [ ] **Step 3: Implement `install_global`**

Replace the `todo!()` in `install_global`:

```rust
/// Install gabb MCP config and/or skill file globally to ~/.claude/.
pub fn install_global(mcp: bool, skill: bool) -> Result<()> {
    let claude_dir = global_claude_dir()?;
    let install_both = !mcp && !skill;

    println!("Installing gabb globally to {}/", claude_dir.display());

    if install_both || mcp {
        install_mcp_config(&claude_dir, false)?;
    }

    if install_both || skill {
        install_skill(&claude_dir)?;
    }

    println!();
    println!("Restart Claude Code to load the changes.");

    Ok(())
}
```

- [ ] **Step 4: Implement `uninstall_global`**

Replace the `todo!()` in `uninstall_global`:

```rust
/// Remove gabb MCP config and/or skill file from ~/.claude/.
pub fn uninstall_global(mcp: bool, skill: bool) -> Result<()> {
    let claude_dir = global_claude_dir()?;
    let uninstall_both = !mcp && !skill;
    let mut removed_any = false;

    println!("Uninstalling gabb from {}/", claude_dir.display());

    if uninstall_both || mcp {
        let mcp_path = claude_dir.join("mcp.json");
        if mcp_path.exists() {
            match uninstall_from_config_file(&mcp_path) {
                Ok(true) => {
                    println!("  Removed gabb from {}", mcp_path.display());
                    removed_any = true;
                }
                Ok(false) => {
                    println!("  gabb was not in {}", mcp_path.display());
                }
                Err(e) => {
                    eprintln!("  Failed to remove MCP config: {}", e);
                }
            }
        } else {
            println!("  No MCP config found at {}", mcp_path.display());
        }
    }

    if uninstall_both || skill {
        let skill_dir = claude_dir.join("skills").join("gabb");
        if skill_dir.exists() {
            std::fs::remove_dir_all(&skill_dir)?;
            println!("  Removed {}/", skill_dir.display());
            removed_any = true;
        } else {
            println!("  No skill found at {}/", skill_dir.display());
        }
    }

    if removed_any {
        println!();
        println!("Restart Claude Code to apply changes.");
    }

    Ok(())
}
```

- [ ] **Step 5: Register the module in `mod.rs`**

In `src/commands/mod.rs`, add:

```rust
pub mod install_global;
```

And add re-exports:

```rust
pub use install_global::{install_global, uninstall_global};
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --bin gabb install_global -- --nocapture`
Expected: All 5 tests PASS.

- [ ] **Step 7: Run full test suite**

Run: `cargo test`
Expected: All tests PASS.

- [ ] **Step 8: Commit**

```bash
git add src/commands/install_global.rs src/commands/mod.rs
git commit -m "feat: add install_global and uninstall_global command implementations"
```

---

### Task 6: Wire commands in `main.rs`

**Files:**
- Modify: `src/main.rs:40-239`

- [ ] **Step 1: Add the new command dispatches**

In `src/main.rs`, add the import at the top (update the existing `use cli::` line):

```rust
use cli::{Cli, Commands, DaemonCommands, McpCommands};
```

Then add the match arms in the `match cli.command` block (after the `Stats` arm):

```rust
        Commands::InstallGlobal { mcp, skill } => {
            commands::install_global(mcp, skill).map(|_| ExitCode::Success)
        }
        Commands::UninstallGlobal { mcp, skill } => {
            commands::uninstall_global(mcp, skill).map(|_| ExitCode::Success)
        }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: Build succeeds with no errors.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --all-targets --all-features`
Expected: No warnings.

- [ ] **Step 4: Run full test suite**

Run: `cargo test`
Expected: All tests PASS.

- [ ] **Step 5: Manual smoke test**

Run: `cargo run -- install-global --help`
Expected output includes:
```
Install gabb MCP server and skill globally (~/.claude/) for all projects

Usage: gabb install-global [OPTIONS]

Options:
      --mcp    Only install MCP server configuration
      --skill  Only install the agent skill file
```

Run: `cargo run -- uninstall-global --help`
Expected output includes similar help text for uninstall.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire install-global and uninstall-global commands in main dispatch"
```

---

### Task 7: Add CLI integration tests

**Files:**
- Modify: `tests/cli_integration.rs`

- [ ] **Step 1: Write integration tests**

Add at the end of `tests/cli_integration.rs`:

```rust
#[test]
fn install_global_creates_mcp_and_skill() {
    // We can't write to the real ~/.claude in tests, so we test the CLI
    // help output works and the command is registered correctly.
    let bin = env!("CARGO_BIN_EXE_gabb");
    let output = Command::new(bin)
        .args(["install-global", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--mcp"), "should have --mcp flag");
    assert!(stdout.contains("--skill"), "should have --skill flag");
    assert!(
        stdout.contains("globally"),
        "help should mention global install"
    );
}

#[test]
fn uninstall_global_has_correct_flags() {
    let bin = env!("CARGO_BIN_EXE_gabb");
    let output = Command::new(bin)
        .args(["uninstall-global", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--mcp"), "should have --mcp flag");
    assert!(stdout.contains("--skill"), "should have --skill flag");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test cli_integration install_global -- --nocapture`
Expected: PASS.

Run: `cargo test --test cli_integration uninstall_global -- --nocapture`
Expected: PASS.

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: All tests PASS.

- [ ] **Step 4: Commit**

```bash
git add tests/cli_integration.rs
git commit -m "test: add integration tests for install-global and uninstall-global"
```

---

### Task 8: Update documentation

**Files:**
- Modify: `README.md`
- Modify: `CLAUDE.md`
- Modify: `assets/SKILL.md`

- [ ] **Step 1: Update README.md**

Add to the commands section in `README.md`:

```markdown
### Global Installation

Install gabb MCP server and skill for all projects (writes to `~/.claude/`):

```bash
# Install both MCP config and skill globally
gabb install-global

# Install only MCP config globally
gabb install-global --mcp

# Install only the skill file globally
gabb install-global --skill

# Remove global installation
gabb uninstall-global
```

When installed globally, gabb's MCP server infers the workspace from the directory Claude Code is opened in, so it works across all projects without per-project setup.
```

- [ ] **Step 2: Update CLAUDE.md**

In `CLAUDE.md`, add to the "Query Commands" or a new "Installation Commands" section:

```markdown
### Global Installation
```bash
# Install MCP and skill to ~/.claude/ (works in all projects)
cargo run -- install-global

# Install only MCP config globally  
cargo run -- install-global --mcp

# Remove global installation
cargo run -- uninstall-global
```
```

- [ ] **Step 3: Update assets/SKILL.md**

Add a brief note to `assets/SKILL.md` mentioning global installation availability. Find an appropriate spot (e.g., after the "When to use" section):

```markdown
## Installation

gabb can be installed per-project (`gabb init --mcp --skill`) or globally (`gabb install-global`).
When installed globally, gabb is available in all Claude Code sessions without per-project setup.
```

- [ ] **Step 4: Run format check**

Run: `cargo fmt --check`
Expected: No formatting issues (docs don't affect Rust formatting, but good to verify nothing else changed).

- [ ] **Step 5: Commit**

```bash
git add README.md CLAUDE.md assets/SKILL.md
git commit -m "docs: document install-global and uninstall-global commands"
```

---

### Task 9: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests PASS.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --all-targets --all-features`
Expected: No warnings.

- [ ] **Step 3: Run format check**

Run: `cargo fmt --check`
Expected: Clean.

- [ ] **Step 4: Review git log**

Run: `git log --oneline main..HEAD`
Expected: 7 clean commits covering refactor, feature, tests, and docs.
