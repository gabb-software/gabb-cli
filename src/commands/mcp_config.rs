//! MCP configuration commands: config, install, status, uninstall, command.

use anyhow::{anyhow, Context, Result};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::cli::McpConfigFormat;

// ==================== Path Helpers ====================

/// Get the path to Claude Desktop config file
pub fn claude_desktop_config_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .map(|h| h.join("Library/Application Support/Claude/claude_desktop_config.json"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA")
            .ok()
            .map(|appdata| PathBuf::from(appdata).join("Claude/claude_desktop_config.json"))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::home_dir().map(|h| h.join(".config/Claude/claude_desktop_config.json"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

/// Get the path to project-level Claude Code MCP config
pub fn claude_code_config_path() -> PathBuf {
    PathBuf::from(".claude/mcp.json")
}

/// Find the gabb binary path
pub fn find_gabb_binary() -> String {
    // Try to find the binary in common locations
    if let Ok(current_exe) = std::env::current_exe() {
        return current_exe.to_string_lossy().to_string();
    }
    // Fallback to just "gabb" (assume it's in PATH)
    "gabb".to_string()
}

/// Generate MCP server config JSON for a workspace
pub fn generate_mcp_config(root: &Path, use_absolute_path: bool) -> serde_json::Value {
    let root_str = if use_absolute_path {
        root.canonicalize()
            .unwrap_or_else(|_| root.to_path_buf())
            .to_string_lossy()
            .to_string()
    } else {
        ".".to_string()
    };

    serde_json::json!({
        "mcpServers": {
            "gabb": {
                "command": find_gabb_binary(),
                "args": ["mcp-server", "--workspace", root_str]
            }
        }
    })
}

// ==================== Commands ====================

/// Print MCP configuration JSON
pub fn mcp_config(root: &Path, format: McpConfigFormat) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let config = generate_mcp_config(&root, true);

    match format {
        McpConfigFormat::Json => {
            // Raw JSON only - suitable for piping/scripting
            println!(
                "{}",
                serde_json::to_string_pretty(&config).unwrap_or_default()
            );
        }
        McpConfigFormat::Snippet => {
            // Friendly output with instructions
            println!("Add this to your Claude Desktop config:\n");
            println!(
                "{}",
                serde_json::to_string_pretty(&config).unwrap_or_default()
            );
            println!();

            if let Some(config_path) = claude_desktop_config_path() {
                println!("Config file location:");
                println!("  {}", config_path.display());
            } else {
                println!("Config file locations:");
                println!(
                    "  macOS:   ~/Library/Application Support/Claude/claude_desktop_config.json"
                );
                println!("  Windows: %APPDATA%\\Claude\\claude_desktop_config.json");
                println!("  Linux:   ~/.config/Claude/claude_desktop_config.json");
            }

            println!();
            println!("Or run `gabb mcp install` to install automatically.");
        }
    }

    Ok(())
}

/// Install gabb into MCP configuration
pub fn mcp_install(root: &Path, claude_desktop_only: bool, claude_code_only: bool) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let install_both = !claude_desktop_only && !claude_code_only;
    let mut installed_any = false;

    // Install to Claude Desktop
    if install_both || claude_desktop_only {
        if let Some(config_path) = claude_desktop_config_path() {
            match install_to_config_file(&config_path, &root, true) {
                Ok(true) => {
                    println!("✓ Installed gabb to Claude Desktop config");
                    println!("  {}", config_path.display());
                    installed_any = true;
                }
                Ok(false) => {
                    println!("✓ gabb already configured in Claude Desktop");
                }
                Err(e) => {
                    eprintln!("✗ Failed to install to Claude Desktop: {}", e);
                }
            }
        } else {
            println!("⚠ Claude Desktop config path not found on this platform");
        }
    }

    // Install to Claude Code (project-level)
    if install_both || claude_code_only {
        let config_path = claude_code_config_path();
        match install_to_config_file(&config_path, &root, false) {
            Ok(true) => {
                println!("✓ Installed gabb to Claude Code project config");
                println!("  {}", config_path.display());
                installed_any = true;
            }
            Ok(false) => {
                println!("✓ gabb already configured in Claude Code project config");
            }
            Err(e) => {
                eprintln!("✗ Failed to install to Claude Code: {}", e);
            }
        }
    }

    if installed_any {
        println!();
        println!("Restart Claude Desktop/Code to load the new MCP server.");
    }

    Ok(())
}

/// Install gabb config to a specific config file
/// Returns Ok(true) if installed, Ok(false) if already present
fn install_to_config_file(config_path: &Path, root: &Path, use_absolute: bool) -> Result<bool> {
    // Create parent directory if needed
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Read existing config or create new one
    let mut config: serde_json::Value = if config_path.exists() {
        let content = fs::read_to_string(config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Check if gabb is already configured
    if config
        .get("mcpServers")
        .and_then(|s| s.get("gabb"))
        .is_some()
    {
        return Ok(false);
    }

    // Backup existing config
    if config_path.exists() {
        let backup_path = config_path.with_extension("json.bak");
        fs::copy(config_path, &backup_path)
            .with_context(|| format!("Failed to backup {}", config_path.display()))?;
    }

    // Add gabb to mcpServers
    let gabb_config = generate_mcp_config(root, use_absolute);
    let mcp_servers = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("Config is not a JSON object"))?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    if let Some(servers) = mcp_servers.as_object_mut() {
        if let Some(gabb) = gabb_config
            .get("mcpServers")
            .and_then(|s| s.get("gabb"))
            .cloned()
        {
            servers.insert("gabb".to_string(), gabb);
        }
    }

    // Write updated config
    let content = serde_json::to_string_pretty(&config)?;
    fs::write(config_path, content)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;

    Ok(true)
}

/// Check MCP configuration status
pub fn mcp_status(workspace: &Path, db: &Path, dry_run: bool) -> Result<()> {
    let mut found_any = false;

    // Check Claude Desktop
    if let Some(config_path) = claude_desktop_config_path() {
        print!("Claude Desktop: ");
        if config_path.exists() {
            match check_gabb_in_config(&config_path) {
                Ok(true) => {
                    println!("✓ gabb configured");
                    println!("  {}", config_path.display());
                    found_any = true;
                }
                Ok(false) => {
                    println!("✗ gabb not configured");
                    println!("  Config exists at: {}", config_path.display());
                }
                Err(e) => {
                    println!("✗ Error reading config: {}", e);
                }
            }
        } else {
            println!("✗ Config file not found");
            println!("  Expected: {}", config_path.display());
        }
    } else {
        println!("Claude Desktop: ⚠ Platform not supported");
    }

    println!();

    // Check Claude Code (project-level)
    let code_config_path = claude_code_config_path();
    print!("Claude Code (project): ");
    if code_config_path.exists() {
        match check_gabb_in_config(&code_config_path) {
            Ok(true) => {
                println!("✓ gabb configured");
                println!("  {}", code_config_path.display());
                found_any = true;
            }
            Ok(false) => {
                println!("✗ gabb not configured");
                println!("  Config exists at: {}", code_config_path.display());
            }
            Err(e) => {
                println!("✗ Error reading config: {}", e);
            }
        }
    } else {
        println!("✗ No project config");
        println!("  Run `gabb mcp install --claude-code` to create");
    }

    println!();

    // Check if gabb binary is accessible
    print!("gabb binary: ");
    if let Ok(exe) = std::env::current_exe() {
        println!("✓ {}", exe.display());
    } else {
        println!("⚠ Could not determine path");
    }

    // Dry-run: test MCP server startup
    if dry_run {
        println!();
        print!("MCP server test: ");

        match test_mcp_server_startup(workspace, db) {
            Ok(()) => {
                println!("✓ Server starts successfully");
            }
            Err(e) => {
                println!("✗ Server startup failed");
                println!("  Error: {}", e);
                return Err(e);
            }
        }
    }

    if !found_any && !dry_run {
        println!();
        println!("Run `gabb mcp install` to configure MCP for Claude.");
    }

    Ok(())
}

/// Test MCP server startup (dry run)
fn test_mcp_server_startup(workspace: &Path, db: &Path) -> Result<()> {
    let gabb_binary = find_gabb_binary();

    // Spawn the MCP server process
    let mut child = Command::new(&gabb_binary)
        .args([
            "mcp-server",
            "--workspace",
            workspace.to_string_lossy().as_ref(),
            "--db",
            db.to_string_lossy().as_ref(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn MCP server: {}", gabb_binary))?;

    // Send an initialize request to test the server responds correctly
    let stdin = child.stdin.as_mut().context("Failed to get stdin")?;
    let initialize_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "gabb-dry-run",
                "version": "1.0.0"
            }
        }
    });

    // Write the request as a single JSON line (gabb MCP server uses JSON lines protocol)
    let request_str = serde_json::to_string(&initialize_request)?;
    writeln!(stdin, "{}", request_str)?;
    stdin.flush()?;

    // Read response with timeout using a channel
    let stdout = child.stdout.take().context("Failed to get stdout")?;
    let (tx, rx) = mpsc::channel::<Result<serde_json::Value>>();

    let reader_thread = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);

        // Read a single JSON line response
        let mut response_line = String::new();
        if reader.read_line(&mut response_line).is_err() {
            let _ = tx.send(Err(anyhow!("Failed to read response")));
            return;
        }

        if response_line.is_empty() {
            let _ = tx.send(Err(anyhow!("Empty response from server")));
            return;
        }

        match serde_json::from_str(&response_line) {
            Ok(response) => {
                let _ = tx.send(Ok(response));
            }
            Err(e) => {
                let _ = tx.send(Err(anyhow!(
                    "Failed to parse response JSON: {} (got: {})",
                    e,
                    response_line.trim()
                )));
            }
        }
    });

    // Wait for response with 5 second timeout
    let result = rx.recv_timeout(Duration::from_secs(5));

    // Kill the server process
    let _ = child.kill();
    let _ = child.wait();

    // Wait for reader thread to finish
    let _ = reader_thread.join();

    // Check result
    match result {
        Ok(Ok(response)) => {
            // Verify it's a valid initialize response
            if response.get("result").is_some() {
                Ok(())
            } else if let Some(error) = response.get("error") {
                Err(anyhow!("Server returned error: {}", error))
            } else {
                Err(anyhow!("Unexpected response format"))
            }
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err(anyhow!("Server did not respond within 5 seconds")),
    }
}

/// Check if gabb is configured in a config file
fn check_gabb_in_config(config_path: &Path) -> Result<bool> {
    let content = fs::read_to_string(config_path)?;
    let config: serde_json::Value = serde_json::from_str(&content)?;
    Ok(config
        .get("mcpServers")
        .and_then(|s| s.get("gabb"))
        .is_some())
}

/// Uninstall gabb from MCP configuration
pub fn mcp_uninstall(claude_desktop_only: bool, claude_code_only: bool) -> Result<()> {
    let uninstall_both = !claude_desktop_only && !claude_code_only;
    let mut removed_any = false;

    // Uninstall from Claude Desktop
    if uninstall_both || claude_desktop_only {
        if let Some(config_path) = claude_desktop_config_path() {
            if config_path.exists() {
                match uninstall_from_config_file(&config_path) {
                    Ok(true) => {
                        println!("✓ Removed gabb from Claude Desktop config");
                        removed_any = true;
                    }
                    Ok(false) => {
                        println!("✓ gabb was not in Claude Desktop config");
                    }
                    Err(e) => {
                        eprintln!("✗ Failed to uninstall from Claude Desktop: {}", e);
                    }
                }
            } else {
                println!("✓ Claude Desktop config does not exist");
            }
        }
    }

    // Uninstall from Claude Code
    if uninstall_both || claude_code_only {
        let config_path = claude_code_config_path();
        if config_path.exists() {
            match uninstall_from_config_file(&config_path) {
                Ok(true) => {
                    println!("✓ Removed gabb from Claude Code project config");
                    removed_any = true;
                }
                Ok(false) => {
                    println!("✓ gabb was not in Claude Code project config");
                }
                Err(e) => {
                    eprintln!("✗ Failed to uninstall from Claude Code: {}", e);
                }
            }
        } else {
            println!("✓ Claude Code project config does not exist");
        }
    }

    if removed_any {
        println!();
        println!("Restart Claude Desktop/Code to apply changes.");
    }

    Ok(())
}

/// Remove gabb from a config file
/// Returns Ok(true) if removed, Ok(false) if wasn't present
fn uninstall_from_config_file(config_path: &Path) -> Result<bool> {
    let content = fs::read_to_string(config_path)?;
    let mut config: serde_json::Value = serde_json::from_str(&content)?;

    // Check if gabb exists
    let has_gabb = config
        .get("mcpServers")
        .and_then(|s| s.get("gabb"))
        .is_some();

    if !has_gabb {
        return Ok(false);
    }

    // Backup before modifying
    let backup_path = config_path.with_extension("json.bak");
    fs::copy(config_path, &backup_path)?;

    // Remove gabb from mcpServers
    if let Some(mcp_servers) = config.get_mut("mcpServers").and_then(|s| s.as_object_mut()) {
        mcp_servers.remove("gabb");
    }

    // Write updated config
    let content = serde_json::to_string_pretty(&config)?;
    fs::write(config_path, content)?;

    Ok(true)
}

/// Generate a slash command file for Claude Code
pub fn mcp_command(root: &Path) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    // Create .claude/commands directory
    let commands_dir = root.join(".claude").join("commands");
    if !commands_dir.exists() {
        fs::create_dir_all(&commands_dir)?;
        println!("Created .claude/commands/");
    }

    let command_file = commands_dir.join("gabb.md");

    // Generate the slash command content
    let content = r#"---
description: Search code symbols with gabb
---
Use the gabb MCP tools to help with this code navigation request.

Available tools:
- gabb_symbols: List/search symbols (functions, classes, types)
- gabb_symbol: Get detailed information about a symbol by name
- gabb_definition: Go to where a symbol is defined
- gabb_usages: Find all references to a symbol
- gabb_implementations: Find implementations of interfaces/traits
- gabb_duplicates: Find duplicate code in the codebase
- gabb_structure: Get hierarchical file structure showing symbols with positions
- gabb_includers: Find all files that #include a header (C++ reverse dependency)
- gabb_includes: Find all headers included by a file (C++ forward dependency)
- gabb_daemon_status: Check if the indexing daemon is running
- gabb_stats: Get index statistics (files by language, symbols by kind, index size)

If the index doesn't exist, gabb will auto-start the daemon to build it.
"#;

    if command_file.exists() {
        println!("Slash command already exists at {}", command_file.display());
        println!("To overwrite, delete it first and run this command again.");
    } else {
        fs::write(&command_file, content)?;
        println!("Created slash command: {}", command_file.display());
        println!();
        println!("You can now use /gabb in Claude Code to invoke gabb tools.");
    }

    Ok(())
}
