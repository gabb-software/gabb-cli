//! Install/uninstall gabb globally to ~/.claude/ for all projects.

use anyhow::Result;

use crate::commands::init::{install_mcp_config, install_skill};
use crate::commands::mcp_config::{global_claude_dir, uninstall_from_config_file};

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
        let args = content["mcpServers"]["gabb"]["args"].as_array().unwrap();
        assert_eq!(
            args.len(),
            1,
            "global config should only have ['mcp-server'], got {:?}",
            args
        );
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
        )
        .unwrap();

        install_mcp_config(&claude_dir, false).unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(claude_dir.join("mcp.json")).unwrap())
                .unwrap();

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
        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&mcp_path).unwrap()).unwrap();
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
