//! Setup wizard command for interactive one-command onboarding.

use anyhow::Result;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

use gabb_cli::daemon;
use gabb_cli::store::IndexStats;

use crate::commands::init::{init_gitignore, init_mcp_config, init_skill};

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

    // Step 5: Offer to update .gitignore
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

    // Step 6: Run initial index (unless --no-index or dry-run)
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

    // Step 7: Print success message and instructions
    println!();
    if dry_run {
        println!("Dry run complete. No changes were made.");
    } else {
        println!("Setup complete! Claude can now use gabb tools.");
        if install_mcp || install_skill {
            println!("Restart Claude Code to load the new configuration.");
        }
        println!();
        println!("To keep the index updated as you work, run:");
        println!("   gabb daemon start --background");
    }

    Ok(())
}
