//! MCP (Model Context Protocol) server implementation.
//!
//! This module implements an MCP server that exposes gabb's code indexing
//! capabilities as tools for AI assistants like Claude.
//!
//! Supports dynamic workspace detection - workspaces are automatically inferred
//! from file paths passed to tools, enabling one MCP server to handle multiple
//! projects.

use crate::store::{IndexStore, SymbolRecord};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

/// Workspace markers - files/directories that indicate a project root
const WORKSPACE_MARKERS: &[&str] = &[
    ".git",
    ".gabb",
    "Cargo.toml",
    "package.json",
    "go.mod",
    "settings.gradle",
    "settings.gradle.kts",
    "pyproject.toml",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
];

/// Directory markers - directories that indicate a project root
const WORKSPACE_DIR_MARKERS: &[&str] = &["gradle", ".git"];

/// Maximum number of workspaces to cache (LRU eviction)
const MAX_CACHED_WORKSPACES: usize = 5;

/// MCP Protocol version
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Server info
const SERVER_NAME: &str = "gabb";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// ==================== JSON-RPC Types ====================

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// JSON-RPC error codes
const PARSE_ERROR: i32 = -32700;
const INTERNAL_ERROR: i32 = -32603;

// ==================== MCP Types ====================

#[derive(Debug, Serialize)]
struct Tool {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

#[derive(Debug, Serialize)]
struct ToolResult {
    content: Vec<ToolContent>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    is_error: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ToolContent {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

impl ToolResult {
    fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent {
                content_type: "text".to_string(),
                text: text.into(),
            }],
            is_error: None,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent {
                content_type: "text".to_string(),
                text: message.into(),
            }],
            is_error: Some(true),
        }
    }
}

// ==================== MCP Server ====================

/// Cached workspace information
struct WorkspaceInfo {
    root: PathBuf,
    db_path: PathBuf,
    store: Option<IndexStore>,
    last_used: std::time::Instant,
}

/// MCP Server state with multi-workspace support
pub struct McpServer {
    /// Default workspace (from --root flag), used when no file path is provided
    default_workspace: PathBuf,
    /// Default database path
    default_db_path: PathBuf,
    /// Cache of workspace -> store mappings
    workspace_cache: HashMap<PathBuf, WorkspaceInfo>,
    /// Whether the MCP client has sent initialized notification
    initialized: bool,
}

impl McpServer {
    pub fn new(workspace_root: PathBuf, db_path: PathBuf) -> Self {
        Self {
            default_workspace: workspace_root,
            default_db_path: db_path,
            workspace_cache: HashMap::new(),
            initialized: false,
        }
    }

    /// Run the MCP server, reading from stdin and writing to stdout.
    pub fn run(&mut self) -> Result<()> {
        let stdin = io::stdin();
        let mut stdout = io::stdout();

        for line in stdin.lock().lines() {
            let line = line?;
            if line.is_empty() {
                continue;
            }

            let response = self.handle_message(&line);
            if let Some(response) = response {
                let json = serde_json::to_string(&response)?;
                writeln!(stdout, "{}", json)?;
                stdout.flush()?;
            }
        }

        Ok(())
    }

    fn handle_message(&mut self, line: &str) -> Option<JsonRpcResponse> {
        // Parse the JSON-RPC request
        let request: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(req) => req,
            Err(e) => {
                return Some(JsonRpcResponse::error(
                    Value::Null,
                    PARSE_ERROR,
                    format!("Parse error: {}", e),
                ));
            }
        };

        // Notifications don't get responses
        let id = match request.id {
            Some(id) => id,
            None => {
                // Handle notification (no response needed)
                self.handle_notification(&request.method, &request.params);
                return None;
            }
        };

        // Handle the request
        match self.handle_request(&request.method, &request.params) {
            Ok(result) => Some(JsonRpcResponse::success(id, result)),
            Err(e) => Some(JsonRpcResponse::error(id, INTERNAL_ERROR, e.to_string())),
        }
    }

    fn handle_notification(&mut self, method: &str, _params: &Value) {
        match method {
            "notifications/initialized" => {
                self.initialized = true;
                log::info!("MCP client initialized");
            }
            _ => {
                log::debug!("Unknown notification: {}", method);
            }
        }
    }

    fn handle_request(&mut self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "initialize" => self.handle_initialize(params),
            "tools/list" => self.handle_tools_list(),
            "tools/call" => self.handle_tools_call(params),
            _ => bail!("Method not found: {}", method),
        }
    }

    fn handle_initialize(&mut self, _params: &Value) -> Result<Value> {
        // Ensure default workspace index is available (auto-start daemon if needed)
        let default_workspace = self.default_workspace.clone();
        self.ensure_workspace_index(&default_workspace)?;

        Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {
                    "listChanged": false
                }
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION
            }
        }))
    }

    fn handle_tools_list(&self) -> Result<Value> {
        let tools = vec![
            Tool {
                name: "gabb_symbols".to_string(),
                description: concat!(
                    "Search for code symbols (functions, classes, interfaces, types, structs, enums, traits) in the indexed codebase. ",
                    "USE THIS INSTEAD OF grep/ripgrep when: finding where a function or class is defined, ",
                    "exploring what methods/functions exist, listing symbols in a file, or searching by symbol kind. ",
                    "Returns precise file:line:column locations. Faster and more accurate than text search for code navigation. ",
                    "Supports TypeScript, Rust, and Kotlin."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Filter by symbol name (exact match). Use this when you know the name you're looking for."
                        },
                        "kind": {
                            "type": "string",
                            "description": "Filter by symbol kind: function, class, interface, type, struct, enum, trait, method, const, variable"
                        },
                        "file": {
                            "type": "string",
                            "description": "Filter to symbols in this file path. Use to explore a specific file's structure."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results (default: 50). Increase for comprehensive searches."
                        }
                    }
                }),
            },
            Tool {
                name: "gabb_symbol".to_string(),
                description: concat!(
                    "Get detailed information about a symbol when you know its name. ",
                    "USE THIS when you have a specific symbol name and want to find where it's defined. ",
                    "Returns the symbol's location, kind, visibility, and container. ",
                    "For exploring unknown code, use gabb_symbols instead."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "The exact symbol name to look up (e.g., 'MyClass', 'process_data', 'UserService')"
                        },
                        "kind": {
                            "type": "string",
                            "description": "Optionally filter by kind if the name is ambiguous (function, class, interface, etc.)"
                        }
                    },
                    "required": ["name"]
                }),
            },
            Tool {
                name: "gabb_definition".to_string(),
                description: concat!(
                    "Jump from a symbol usage to its definition/declaration. ",
                    "USE THIS when you see a function call, type reference, or variable and want to see where it's defined. ",
                    "Works across files and through imports. Provide the file and position where the symbol is USED, ",
                    "and this returns where it's DEFINED. Essential for understanding unfamiliar code."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Path to the file containing the symbol usage (absolute or relative to workspace)"
                        },
                        "line": {
                            "type": "integer",
                            "description": "1-based line number where the symbol appears"
                        },
                        "character": {
                            "type": "integer",
                            "description": "1-based column number (position within the line)"
                        }
                    },
                    "required": ["file", "line", "character"]
                }),
            },
            Tool {
                name: "gabb_usages".to_string(),
                description: concat!(
                    "Find ALL places where a symbol is used/referenced across the codebase. ",
                    "USE THIS BEFORE REFACTORING to understand impact, when investigating how a function is called, ",
                    "or to find all consumers of an API. More accurate than text search - understands code structure ",
                    "and won't match comments or strings. Point to a symbol definition to find all its usages."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Path to the file containing the symbol definition"
                        },
                        "line": {
                            "type": "integer",
                            "description": "1-based line number of the symbol"
                        },
                        "character": {
                            "type": "integer",
                            "description": "1-based column number within the line"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum usages to return (default: 50). Increase for thorough analysis."
                        }
                    },
                    "required": ["file", "line", "character"]
                }),
            },
            Tool {
                name: "gabb_implementations".to_string(),
                description: concat!(
                    "Find all implementations of an interface, trait, or abstract class. ",
                    "USE THIS when you have an interface/trait and want to find concrete implementations, ",
                    "or when exploring a codebase's architecture to understand what classes implement a contract. ",
                    "Point to the interface/trait definition to find all implementing classes/structs."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Path to the file containing the interface/trait definition"
                        },
                        "line": {
                            "type": "integer",
                            "description": "1-based line number of the interface/trait"
                        },
                        "character": {
                            "type": "integer",
                            "description": "1-based column number within the line"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum implementations to return (default: 50)"
                        }
                    },
                    "required": ["file", "line", "character"]
                }),
            },
            Tool {
                name: "gabb_daemon_status".to_string(),
                description: concat!(
                    "Check if the gabb indexing daemon is running and get workspace info. ",
                    "USE THIS to diagnose issues if other gabb tools aren't working, ",
                    "or to verify the index is up-to-date. Returns daemon PID, version, and index location."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ];

        Ok(json!({ "tools": tools }))
    }

    fn handle_tools_call(&mut self, params: &Value) -> Result<Value> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;

        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let result = match name {
            "gabb_symbols" => self.tool_symbols(&arguments),
            "gabb_symbol" => self.tool_symbol(&arguments),
            "gabb_definition" => self.tool_definition(&arguments),
            "gabb_usages" => self.tool_usages(&arguments),
            "gabb_implementations" => self.tool_implementations(&arguments),
            "gabb_daemon_status" => self.tool_daemon_status(),
            _ => Ok(ToolResult::error(format!("Unknown tool: {}", name))),
        }?;

        Ok(serde_json::to_value(result)?)
    }

    // ==================== Workspace Management ====================

    /// Infer workspace root from a file path by walking up to find markers
    fn infer_workspace(&self, file_path: &Path) -> Option<PathBuf> {
        let file_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            self.default_workspace.join(file_path)
        };

        let mut current = file_path.parent()?;

        loop {
            // Check file markers
            for marker in WORKSPACE_MARKERS {
                if current.join(marker).exists() {
                    return Some(current.to_path_buf());
                }
            }

            // Check directory markers
            for marker in WORKSPACE_DIR_MARKERS {
                let marker_path = current.join(marker);
                if marker_path.is_dir() {
                    return Some(current.to_path_buf());
                }
            }

            // Move up to parent directory
            current = current.parent()?;
        }
    }

    /// Get or create workspace info for a given workspace root
    fn get_or_create_workspace(&mut self, workspace_root: &Path) -> Result<&mut WorkspaceInfo> {
        let workspace_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());

        // Evict oldest workspace if cache is full
        if !self.workspace_cache.contains_key(&workspace_root)
            && self.workspace_cache.len() >= MAX_CACHED_WORKSPACES
        {
            // Find oldest entry
            if let Some(oldest_key) = self
                .workspace_cache
                .iter()
                .min_by_key(|(_, info)| info.last_used)
                .map(|(k, _)| k.clone())
            {
                log::debug!("Evicting workspace from cache: {}", oldest_key.display());
                self.workspace_cache.remove(&oldest_key);
            }
        }

        // Insert if not present
        if !self.workspace_cache.contains_key(&workspace_root) {
            let db_path = workspace_root.join(".gabb/index.db");
            log::debug!(
                "Adding workspace to cache: {} (db: {})",
                workspace_root.display(),
                db_path.display()
            );
            self.workspace_cache.insert(
                workspace_root.clone(),
                WorkspaceInfo {
                    root: workspace_root.clone(),
                    db_path,
                    store: None,
                    last_used: std::time::Instant::now(),
                },
            );
        }

        // Update last_used and return
        let info = self.workspace_cache.get_mut(&workspace_root).unwrap();
        info.last_used = std::time::Instant::now();
        Ok(info)
    }

    /// Ensure index exists for a workspace, starting daemon if needed
    fn ensure_workspace_index(&mut self, workspace_root: &Path) -> Result<()> {
        use crate::daemon;

        let info = self.get_or_create_workspace(workspace_root)?;

        if info.store.is_some() {
            return Ok(());
        }

        if !info.db_path.exists() {
            // Start daemon in background
            log::info!(
                "Index not found for {}. Starting daemon...",
                info.root.display()
            );
            daemon::start(&info.root, &info.db_path, false, true, None)?;

            // Wait for index to be ready
            let max_wait = std::time::Duration::from_secs(60);
            let start = std::time::Instant::now();
            let db_path = info.db_path.clone();
            while !db_path.exists() && start.elapsed() < max_wait {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }

            if !db_path.exists() {
                bail!("Daemon started but index not created within 60 seconds");
            }
        }

        // Re-get info (borrow checker)
        let info = self.workspace_cache.get_mut(workspace_root).unwrap();
        info.store = Some(IndexStore::open(&info.db_path)?);
        Ok(())
    }

    /// Get workspace root for a file, falling back to default
    fn workspace_for_file(&self, file_path: Option<&str>) -> PathBuf {
        if let Some(path) = file_path {
            let path = PathBuf::from(path);
            if let Some(workspace) = self.infer_workspace(&path) {
                return workspace;
            }
        }
        self.default_workspace.clone()
    }

    /// Get store for a workspace (ensures index exists)
    fn get_store_for_workspace(&mut self, workspace_root: &Path) -> Result<&IndexStore> {
        self.ensure_workspace_index(workspace_root)?;
        let info = self.workspace_cache.get(workspace_root).unwrap();
        info.store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Store not initialized"))
    }

    // ==================== Tool Implementations ====================

    fn tool_symbols(&mut self, args: &Value) -> Result<ToolResult> {
        let name = args.get("name").and_then(|v| v.as_str());
        let kind = args.get("kind").and_then(|v| v.as_str());
        let file = args.get("file").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        // Infer workspace from file path if provided
        let workspace = self.workspace_for_file(file);
        let store = self.get_store_for_workspace(&workspace)?;

        let symbols = store.list_symbols(file, kind, name, Some(limit))?;

        if symbols.is_empty() {
            return Ok(ToolResult::text("No symbols found matching the criteria."));
        }

        let output = format_symbols(&symbols, &workspace);
        Ok(ToolResult::text(output))
    }

    fn tool_symbol(&mut self, args: &Value) -> Result<ToolResult> {
        // Use default workspace since we don't have a file path
        let workspace = self.default_workspace.clone();
        let store = self.get_store_for_workspace(&workspace)?;

        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'name' argument"))?;

        let kind = args.get("kind").and_then(|v| v.as_str());

        let symbols = store.list_symbols(None, kind, Some(name), Some(10))?;

        if symbols.is_empty() {
            return Ok(ToolResult::text(format!(
                "No symbol found with name '{}'",
                name
            )));
        }

        let output = format_symbols(&symbols, &workspace);
        Ok(ToolResult::text(output))
    }

    fn tool_definition(&mut self, args: &Value) -> Result<ToolResult> {
        let file = args
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'file' argument"))?;
        let line = args
            .get("line")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'line' argument"))? as usize;
        let character = args
            .get("character")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'character' argument"))?
            as usize;

        // Infer workspace from file path
        let workspace = self.workspace_for_file(Some(file));
        let file_path = self.resolve_path_for_workspace(file, &workspace);

        // Find symbol at position
        if let Some(symbol) = self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)? {
            let output = format_symbol(&symbol, &workspace);
            return Ok(ToolResult::text(format!("Definition:\n{}", output)));
        }

        Ok(ToolResult::text(format!(
            "No symbol found at {}:{}:{}",
            file, line, character
        )))
    }

    fn tool_usages(&mut self, args: &Value) -> Result<ToolResult> {
        let file = args
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'file' argument"))?;
        let line = args
            .get("line")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'line' argument"))? as usize;
        let character = args
            .get("character")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'character' argument"))?
            as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        // Infer workspace from file path
        let workspace = self.workspace_for_file(Some(file));
        let file_path = self.resolve_path_for_workspace(file, &workspace);

        // Find symbol at position
        let symbol = match self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)? {
            Some(s) => s,
            None => {
                return Ok(ToolResult::text(format!(
                    "No symbol found at {}:{}:{}",
                    file, line, character
                )));
            }
        };

        // Find references using references_for_symbol
        let store = self.get_store_for_workspace(&workspace)?;
        let refs = store.references_for_symbol(&symbol.id)?;

        if refs.is_empty() {
            return Ok(ToolResult::text(format!(
                "No usages found for '{}'",
                symbol.name
            )));
        }

        let refs: Vec<_> = refs.into_iter().take(limit).collect();
        let mut output = format!("Usages of '{}' ({} found):\n\n", symbol.name, refs.len());
        for r in &refs {
            let rel_path = relative_path_for_workspace(&r.file, &workspace);
            // Convert offset to line:col
            if let Ok((ref_line, ref_col)) = offset_to_line_col(&r.file, r.start as usize) {
                output.push_str(&format!("  {}:{}:{}\n", rel_path, ref_line, ref_col));
            }
        }

        Ok(ToolResult::text(output))
    }

    fn tool_implementations(&mut self, args: &Value) -> Result<ToolResult> {
        let file = args
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'file' argument"))?;
        let line = args
            .get("line")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'line' argument"))? as usize;
        let character = args
            .get("character")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'character' argument"))?
            as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        // Infer workspace from file path
        let workspace = self.workspace_for_file(Some(file));
        let file_path = self.resolve_path_for_workspace(file, &workspace);

        // Find symbol at position
        let symbol = match self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)? {
            Some(s) => s,
            None => {
                return Ok(ToolResult::text(format!(
                    "No symbol found at {}:{}:{}",
                    file, line, character
                )));
            }
        };

        // Find implementations via edges_to (edges pointing TO the symbol from implementations)
        let store = self.get_store_for_workspace(&workspace)?;
        let edges = store.edges_to(&symbol.id)?;
        let impl_ids: Vec<String> = edges.into_iter().map(|e| e.src).collect();
        let mut impls = store.symbols_by_ids(&impl_ids)?;

        if impls.is_empty() {
            // Fallback: search by name
            let fallback = store.list_symbols(None, None, Some(&symbol.name), Some(limit))?;
            if fallback.len() <= 1 {
                return Ok(ToolResult::text(format!(
                    "No implementations found for '{}'",
                    symbol.name
                )));
            }
            let output = format_symbols(&fallback, &workspace);
            return Ok(ToolResult::text(format!(
                "Implementations of '{}' (by name):\n\n{}",
                symbol.name, output
            )));
        }

        impls.truncate(limit);
        let output = format_symbols(&impls, &workspace);
        Ok(ToolResult::text(format!(
            "Implementations of '{}':\n\n{}",
            symbol.name, output
        )))
    }

    fn tool_daemon_status(&mut self) -> Result<ToolResult> {
        use crate::daemon;

        let mut status = String::new();

        // Show default workspace status
        status.push_str("Default Workspace:\n");
        if let Ok(Some(pid_info)) = daemon::read_pid_file(&self.default_workspace) {
            if daemon::is_process_running(pid_info.pid) {
                status.push_str(&format!(
                    "  Daemon: running (PID {})\n  Version: {}\n  Root: {}\n  Database: {}\n",
                    pid_info.pid,
                    pid_info.version,
                    self.default_workspace.display(),
                    self.default_db_path.display()
                ));
            } else {
                status.push_str(&format!(
                    "  Daemon: not running (stale PID file)\n  Root: {}\n  Database: {}\n",
                    self.default_workspace.display(),
                    self.default_db_path.display()
                ));
            }
        } else {
            status.push_str(&format!(
                "  Daemon: not running\n  Root: {}\n  Database: {}\n",
                self.default_workspace.display(),
                self.default_db_path.display()
            ));
        }

        // Show cached workspaces
        if !self.workspace_cache.is_empty() {
            status.push_str(&format!(
                "\nCached Workspaces ({}/{}):\n",
                self.workspace_cache.len(),
                MAX_CACHED_WORKSPACES
            ));
            for (root, info) in &self.workspace_cache {
                let daemon_status = if let Ok(Some(pid_info)) = daemon::read_pid_file(root) {
                    if daemon::is_process_running(pid_info.pid) {
                        format!("running (PID {})", pid_info.pid)
                    } else {
                        "not running".to_string()
                    }
                } else {
                    "not running".to_string()
                };
                let index_status = if info.store.is_some() {
                    "loaded"
                } else if info.db_path.exists() {
                    "available"
                } else {
                    "not indexed"
                };
                status.push_str(&format!(
                    "  {}\n    Daemon: {}, Index: {}\n",
                    root.display(),
                    daemon_status,
                    index_status
                ));
            }
        }

        Ok(ToolResult::text(status))
    }

    // ==================== Helper Methods ====================

    fn resolve_path_for_workspace(&self, path: &str, workspace: &Path) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            workspace.join(p)
        }
    }

    fn find_symbol_at_in_workspace(
        &mut self,
        file: &Path,
        line: usize,
        character: usize,
        workspace: &Path,
    ) -> Result<Option<SymbolRecord>> {
        let store = self.get_store_for_workspace(workspace)?;
        let file_str = file.to_string_lossy().to_string();

        // Convert line:col to byte offset
        let content = std::fs::read_to_string(file)?;
        let mut offset: i64 = 0;
        for (i, l) in content.lines().enumerate() {
            if i + 1 == line {
                offset += character.saturating_sub(1) as i64;
                break;
            }
            offset += l.len() as i64 + 1; // +1 for newline
        }

        // Find symbol containing this offset
        let symbols = store.list_symbols(Some(&file_str), None, None, None)?;

        // Find the narrowest symbol containing the offset
        let mut best: Option<SymbolRecord> = None;
        for sym in symbols {
            if sym.start <= offset && offset < sym.end {
                let span = sym.end - sym.start;
                if best
                    .as_ref()
                    .map(|b| span < (b.end - b.start))
                    .unwrap_or(true)
                {
                    best = Some(sym);
                }
            }
        }

        Ok(best)
    }
}

// ==================== Formatting Helpers ====================

/// Get relative path for a file within a workspace
fn relative_path_for_workspace(path: &str, workspace: &Path) -> String {
    let p = PathBuf::from(path);
    p.strip_prefix(workspace)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}

fn format_symbols(symbols: &[SymbolRecord], workspace_root: &Path) -> String {
    let mut output = String::new();
    for sym in symbols {
        output.push_str(&format_symbol(sym, workspace_root));
        output.push('\n');
    }
    output
}

fn format_symbol(sym: &SymbolRecord, workspace_root: &Path) -> String {
    let rel_path = PathBuf::from(&sym.file)
        .strip_prefix(workspace_root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| sym.file.clone());

    // Convert byte offset to line:col
    let location = if let Ok((line, col)) = offset_to_line_col(&sym.file, sym.start as usize) {
        format!("{}:{}:{}", rel_path, line, col)
    } else {
        format!("{}:offset:{}", rel_path, sym.start)
    };

    let mut parts = vec![format!("{:<10} {:<30} {}", sym.kind, sym.name, location)];

    if let Some(ref vis) = sym.visibility {
        parts.push(format!("  visibility: {}", vis));
    }
    if let Some(ref container) = sym.container {
        parts.push(format!("  container: {}", container));
    }

    parts.join("\n")
}

/// Convert byte offset to 1-based line:column
fn offset_to_line_col(file_path: &str, offset: usize) -> Result<(usize, usize)> {
    let content = std::fs::read(file_path)?;
    let mut line = 1;
    let mut col = 1;
    for (i, &b) in content.iter().enumerate() {
        if i == offset {
            return Ok((line, col));
        }
        if b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    if offset == content.len() {
        Ok((line, col))
    } else {
        anyhow::bail!("offset out of bounds")
    }
}

/// Run the MCP server with the given workspace and database paths.
pub fn run_server(workspace_root: &Path, db_path: &Path) -> Result<()> {
    let mut server = McpServer::new(workspace_root.to_path_buf(), db_path.to_path_buf());
    server.run()
}
