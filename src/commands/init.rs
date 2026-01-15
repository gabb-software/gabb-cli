//! Init command for initializing gabb in a project.

use anyhow::{anyhow, Result};
use std::fs;
use std::path::Path;

use crate::commands::mcp_config::find_gabb_binary;

/// Initialize gabb in a project
pub fn init_project(
    root: &Path,
    setup_mcp: bool,
    setup_gitignore: bool,
    setup_skill: bool,
) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    println!("Initializing gabb in {}", root.display());

    // Create .gabb directory
    let gabb_dir = root.join(".gabb");
    if !gabb_dir.exists() {
        fs::create_dir_all(&gabb_dir)?;
        println!("  Created .gabb/");
    } else {
        println!("  .gabb/ already exists");
    }

    // Setup MCP configuration if requested
    if setup_mcp {
        init_mcp_config(&root)?;
    }

    // Setup .gitignore if requested
    if setup_gitignore {
        init_gitignore(&root)?;
    }

    // Setup agent skill if requested
    if setup_skill {
        init_skill(&root)?;
    }

    println!();
    println!("Next steps:");
    println!("  1. Start the daemon:    gabb daemon start");
    if setup_mcp {
        println!("  2. Restart Claude Code to load the MCP server");
    } else if setup_skill {
        println!("  2. The skill will auto-activate when Claude Code sees relevant requests");
    } else {
        println!("  2. For AI integration: gabb init --mcp");
    }

    Ok(())
}

/// Create .claude/mcp.json with gabb configuration
///
/// This is used by both `init --mcp` and `setup` commands.
pub(crate) fn init_mcp_config(root: &Path) -> Result<()> {
    let claude_dir = root.join(".claude");
    let mcp_config_path = claude_dir.join("mcp.json");

    // Create .claude directory
    if !claude_dir.exists() {
        fs::create_dir_all(&claude_dir)?;
        println!("  Created .claude/");
    }

    // Generate MCP config with relative path (version control friendly)
    let config = serde_json::json!({
        "mcpServers": {
            "gabb": {
                "command": find_gabb_binary(),
                "args": ["mcp-server", "--workspace", "."]
            }
        }
    });

    if mcp_config_path.exists() {
        // Check if gabb already configured
        let existing = fs::read_to_string(&mcp_config_path)?;
        let existing_config: serde_json::Value = serde_json::from_str(&existing)?;
        if existing_config
            .get("mcpServers")
            .and_then(|s| s.get("gabb"))
            .is_some()
        {
            println!("  .claude/mcp.json already has gabb configured");
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
        println!("  Added gabb to .claude/mcp.json");
    } else {
        fs::write(&mcp_config_path, serde_json::to_string_pretty(&config)?)?;
        println!("  Created .claude/mcp.json");
    }

    Ok(())
}

/// Add .gabb/ and .claude/ to .gitignore
///
/// This is used by both `init --gitignore` and `setup` commands.
pub(crate) fn init_gitignore(root: &Path) -> Result<()> {
    let gitignore_path = root.join(".gitignore");
    let entries_to_add = vec![".gabb/", ".claude/"];

    let existing_content = if gitignore_path.exists() {
        fs::read_to_string(&gitignore_path)?
    } else {
        String::new()
    };

    let existing_lines: Vec<&str> = existing_content.lines().collect();
    let mut additions = Vec::new();

    for entry in &entries_to_add {
        // Check if entry already exists (exact match or with comment)
        let already_present = existing_lines.iter().any(|line| {
            let trimmed = line.trim();
            trimmed == *entry || trimmed == entry.trim_end_matches('/')
        });

        if !already_present {
            additions.push(*entry);
        }
    }

    if additions.is_empty() {
        println!("  .gitignore already configured");
        return Ok(());
    }

    // Append to .gitignore
    let mut content = existing_content;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.is_empty() {
        content.push_str("\n# gabb code indexing\n");
    } else {
        content.push_str("# gabb code indexing\n");
    }
    for entry in &additions {
        content.push_str(entry);
        content.push('\n');
    }

    fs::write(&gitignore_path, content)?;
    println!(
        "  Added {} to .gitignore",
        additions
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(())
}

/// Create .claude/skills/gabb/ agent skill for Claude Code discoverability
///
/// This is used by both `init --skill` and `setup` commands.
pub(crate) fn init_skill(root: &Path) -> Result<()> {
    // Create .claude/skills/gabb directory
    let skills_dir = root.join(".claude").join("skills").join("gabb");
    if !skills_dir.exists() {
        fs::create_dir_all(&skills_dir)?;
        println!("  Created .claude/skills/gabb/");
    }

    // Write SKILL.md (self-contained skill file with all guidance)
    let skill_file = skills_dir.join("SKILL.md");
    let skill_content = include_str!("../../assets/SKILL.md");
    write_skill_file(&skill_file, skill_content, "SKILL.md")?;

    Ok(())
}

/// Helper to write a skill file, checking if it needs updating
fn write_skill_file(path: &Path, content: &str, name: &str) -> Result<()> {
    if path.exists() {
        let existing = fs::read_to_string(path)?;
        if existing == content {
            println!("  .claude/skills/gabb/{} is up to date", name);
            return Ok(());
        }
        fs::write(path, content)?;
        println!("  Updated .claude/skills/gabb/{}", name);
    } else {
        fs::write(path, content)?;
        println!("  Created .claude/skills/gabb/{}", name);
        if name == "SKILL.md" {
            println!("  Claude will auto-discover this skill for code navigation tasks");
        }
    }
    Ok(())
}
