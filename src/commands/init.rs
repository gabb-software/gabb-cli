//! Init and setup commands: init, setup wizard.

use anyhow::{anyhow, Result};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

use gabb_cli::daemon;
use gabb_cli::store::IndexStats;

use crate::commands::mcp_config::find_gabb_binary;

/// Marker to detect if CLAUDE.md already has gabb section
const CLAUDEMD_SECTION_MARKER: &str = "## Tool Selection: Use gabb";

// ==================== Init Command ====================

/// Initialize gabb in a project
pub fn init_project(
    root: &Path,
    setup_mcp: bool,
    setup_gitignore: bool,
    setup_skill: bool,
    setup_claudemd: bool,
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

    // Setup CLAUDE.md if requested
    if setup_claudemd {
        init_claudemd(&root)?;
    }

    println!();
    println!("Next steps:");
    println!("  1. Start the daemon:    gabb daemon start");
    if setup_mcp {
        println!("  2. Restart Claude Code to load the MCP server");
    } else if setup_skill {
        println!("  2. The skill will auto-activate when Claude Code sees relevant requests");
    } else if setup_claudemd {
        println!("  2. The CLAUDE.md guidance is now active for Claude Code");
    } else {
        println!("  2. For AI integration: gabb init --mcp --claudemd");
    }

    Ok(())
}

/// Create .claude/mcp.json with gabb configuration
fn init_mcp_config(root: &Path) -> Result<()> {
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
fn init_gitignore(root: &Path) -> Result<()> {
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
fn init_skill(root: &Path) -> Result<()> {
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

/// Add gabb tool guidance section to CLAUDE.md
fn init_claudemd(root: &Path) -> Result<()> {
    let claudemd_path = root.join("CLAUDE.md");
    let section_content = include_str!("../../assets/CLAUDE_SECTION.md");

    if claudemd_path.exists() {
        // Read existing content
        let existing = fs::read_to_string(&claudemd_path)?;

        // Check if section already exists
        if existing.contains(CLAUDEMD_SECTION_MARKER) {
            println!("  CLAUDE.md already has gabb section");
            return Ok(());
        }

        // Append section to existing file
        let new_content = format!("{}\n\n{}", existing.trim_end(), section_content);
        fs::write(&claudemd_path, new_content)?;
        println!("  Added gabb section to CLAUDE.md");
    } else {
        // Create new CLAUDE.md with section
        let header = "# Project Instructions\n\nThis file provides guidance to Claude Code.\n\n";
        let new_content = format!("{}{}", header, section_content);
        fs::write(&claudemd_path, new_content)?;
        println!("  Created CLAUDE.md with gabb tool guidance");
    }

    Ok(())
}

// ==================== Setup Wizard ====================

/// Detect what kind of project this is based on marker files
fn detect_project_type(root: &Path) -> Option<&'static str> {
    let markers = [
        ("Cargo.toml", "Rust"),
        ("package.json", "Node.js"),
        ("pyproject.toml", "Python"),
        ("go.mod", "Go"),
        ("build.gradle", "Gradle"),
        ("build.gradle.kts", "Gradle (Kotlin)"),
        ("pom.xml", "Maven"),
        ("CMakeLists.txt", "CMake"),
        ("Makefile", "Make"),
    ];

    for (file, project_type) in markers {
        if root.join(file).exists() {
            return Some(project_type);
        }
    }
    None
}

/// Prompt user for yes/no confirmation
fn prompt_yes_no(prompt: &str, default_yes: bool) -> Result<bool> {
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{} {} ", prompt, suffix);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    if input.is_empty() {
        return Ok(default_yes);
    }

    Ok(input == "y" || input == "yes")
}

/// Display index statistics in a formatted table for setup wizard
fn display_index_stats_table(stats: &IndexStats) {
    println!();
    println!("Index Statistics:");

    // Collect language stats
    let mut lang_stats: Vec<(&str, i64)> = Vec::new();

    // For each language, get file count
    for (lang, file_count) in &stats.files.by_language {
        lang_stats.push((lang.as_str(), *file_count));
    }

    let total_symbols = stats.symbols.total;

    // Sort by file count descending
    lang_stats.sort_by(|a, b| b.1.cmp(&a.1));

    // Calculate column widths
    let lang_width = lang_stats
        .iter()
        .map(|(l, _)| l.len())
        .max()
        .unwrap_or(8)
        .max(8);

    // Print header
    println!(
        "   {:lang_width$}   {:>8}",
        "Language",
        "Files",
        lang_width = lang_width
    );
    println!("   {}   {}", "-".repeat(lang_width), "-".repeat(8));

    // Print each language
    for (lang, files) in &lang_stats {
        let display_lang = capitalize_language(lang);
        println!(
            "   {:lang_width$}   {:>8}",
            display_lang,
            format_number(*files),
            lang_width = lang_width
        );
    }

    // Print totals
    println!("   {}   {}", "-".repeat(lang_width), "-".repeat(8));
    println!(
        "   {:lang_width$}   {:>8}",
        "Total",
        format_number(stats.files.total),
        lang_width = lang_width
    );
    println!();
    println!("   Symbols: {}", format_number(total_symbols));
}

/// Capitalize language name for display
fn capitalize_language(lang: &str) -> String {
    match lang.to_lowercase().as_str() {
        "typescript" => "TypeScript".to_string(),
        "javascript" => "JavaScript".to_string(),
        "python" => "Python".to_string(),
        "rust" => "Rust".to_string(),
        "kotlin" => "Kotlin".to_string(),
        "cpp" | "c++" => "C++".to_string(),
        "c" => "C".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        }
    }
}

/// Format a number with thousands separators
fn format_number(n: i64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Interactive setup wizard for one-command onboarding
pub fn setup_wizard(
    root: &Path,
    db: &Path,
    yes: bool,
    dry_run: bool,
    no_index: bool,
) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let gabb_dir = root.join(".gabb");

    // Step 1: Detect and display workspace
    println!();
    let project_type = detect_project_type(&root);
    if let Some(ptype) = project_type {
        println!("Detected workspace: {} ({} found)", root.display(), ptype);
    } else {
        println!("Detected workspace: {}", root.display());
    }

    // Step 2: Create .gabb directory
    let gabb_exists = gabb_dir.exists();
    if gabb_exists {
        println!(".gabb/ already exists");
    } else if dry_run {
        println!("Would create .gabb/");
    } else {
        fs::create_dir_all(&gabb_dir)?;
        println!("Created .gabb/");
    }

    // Step 3: Offer to install MCP config for Claude Code
    let claude_dir = root.join(".claude");
    let mcp_config_path = claude_dir.join("mcp.json");
    let mcp_already_configured = if mcp_config_path.exists() {
        let content = fs::read_to_string(&mcp_config_path).unwrap_or_default();
        content.contains("\"gabb\"")
    } else {
        false
    };

    let install_mcp = if mcp_already_configured {
        println!("Claude Code MCP already configured");
        false
    } else {
        let should_install = yes || prompt_yes_no("Install MCP config for Claude Code?", true)?;
        if should_install {
            if dry_run {
                println!("   Would add gabb to .claude/mcp.json");
            } else {
                init_mcp_config(&root)?;
                println!("   Added gabb to Claude Code config");
            }
        }
        should_install
    };

    // Step 4: Offer to add skill file
    let skill_dir = root.join(".claude").join("skills").join("gabb");
    let skill_file = skill_dir.join("SKILL.md");
    let skill_exists = skill_file.exists();

    let install_skill = if skill_exists {
        println!("Agent skill already exists");
        false
    } else {
        let should_install = yes || prompt_yes_no("Create agent skill for Claude?", true)?;
        if should_install {
            if dry_run {
                println!("   Would create .claude/skills/gabb/SKILL.md");
            } else {
                init_skill(&root)?;
                println!("   Created agent skill");
            }
        }
        should_install
    };

    // Step 5: Offer to add CLAUDE.md section
    let claudemd_path = root.join("CLAUDE.md");
    let claudemd_has_gabb = if claudemd_path.exists() {
        let content = fs::read_to_string(&claudemd_path).unwrap_or_default();
        content.contains(CLAUDEMD_SECTION_MARKER)
    } else {
        false
    };

    let install_claudemd = if claudemd_has_gabb {
        println!("CLAUDE.md already has gabb section");
        false
    } else {
        let prompt_msg = if claudemd_path.exists() {
            "Add gabb guidance to CLAUDE.md?"
        } else {
            "Create CLAUDE.md with gabb guidance?"
        };
        let should_install = yes || prompt_yes_no(prompt_msg, true)?;
        if should_install {
            if dry_run {
                if claudemd_path.exists() {
                    println!("   Would add gabb section to CLAUDE.md");
                } else {
                    println!("   Would create CLAUDE.md with gabb guidance");
                }
            } else {
                init_claudemd(&root)?;
            }
        }
        should_install
    };

    // Step 6: Offer to update .gitignore
    let gitignore_path = root.join(".gitignore");
    let gitignore_content = if gitignore_path.exists() {
        fs::read_to_string(&gitignore_path).unwrap_or_default()
    } else {
        String::new()
    };
    let gitignore_has_gabb = gitignore_content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == ".gabb/" || trimmed == ".gabb"
    });
    let gitignore_has_claude = gitignore_content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == ".claude/" || trimmed == ".claude"
    });

    if gitignore_has_gabb && gitignore_has_claude {
        println!(".gitignore already configured");
    } else {
        let should_update = yes || prompt_yes_no("Add .gabb/ and .claude/ to .gitignore?", true)?;
        if should_update {
            if dry_run {
                if !gitignore_has_gabb {
                    println!("   Would add .gabb/ to .gitignore");
                }
                if !gitignore_has_claude {
                    println!("   Would add .claude/ to .gitignore");
                }
            } else {
                init_gitignore(&root)?;
                println!("   Updated .gitignore");
            }
        }
    }

    // Step 7: Run initial index (unless --no-index or dry-run)
    let stats = if no_index {
        println!("Skipping initial index (--no-index)");
        None
    } else if dry_run {
        println!("Would run initial index");
        None
    } else {
        // Check if daemon is already running - if so, index is already available
        if let Ok(Some(pid_info)) = daemon::read_pid_file(&root) {
            if daemon::is_process_running(pid_info.pid) {
                println!(
                    "Daemon already running (PID {}), index available",
                    pid_info.pid
                );
                None
            } else {
                // Run initial indexing with progress
                println!("Running initial index...");
                Some(daemon::run_initial_index(&root, db, false, false)?)
            }
        } else {
            // Run initial indexing with progress
            println!("Running initial index...");
            Some(daemon::run_initial_index(&root, db, false, false)?)
        }
    };

    // Display stats if we indexed
    if let Some(ref stats) = stats {
        display_index_stats_table(stats);
    }

    // Step 8: Print success message and instructions
    println!();
    if dry_run {
        println!("Dry run complete. No changes were made.");
    } else {
        println!("Setup complete! Claude can now use gabb tools.");
        if install_mcp || install_skill || install_claudemd {
            println!("Restart Claude Code to load the new configuration.");
        }
        println!();
        println!("To keep the index updated as you work, run:");
        println!("   gabb daemon start --background");
    }

    Ok(())
}
