//! MCP (Model Context Protocol) server implementation.
//!
//! This module implements an MCP server that exposes gabb's code indexing
//! capabilities as tools for AI assistants like Claude.
//!
//! Supports dynamic workspace detection - workspaces are automatically inferred
//! from file paths passed to tools, enabling one MCP server to handle multiple
//! projects.

// Allow dead code for tool implementations that are kept but not currently exposed via MCP.
// These can be re-enabled later by adding them back to handle_tools_list/handle_tools_call.
#![allow(dead_code)]

use crate::is_test_file;
use crate::store::{DuplicateGroup, IndexStore, SymbolQuery, SymbolRecord};
use crate::workspace;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

/// Options for formatting symbol output
#[derive(Debug, Clone, Default)]
pub struct FormatOptions {
    /// Include the symbol's source code in output
    pub include_source: bool,
    /// Number of context lines before/after the symbol (like grep -C)
    pub context_lines: Option<usize>,
}

/// Maximum number of workspaces to cache (LRU eviction)
const MAX_CACHED_WORKSPACES: usize = 5;

/// MCP Protocol version
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Server info
const SERVER_NAME: &str = "gabb";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// MCP server instructions (sent to client during initialization)
const SERVER_INSTRUCTIONS: &str = include_str!("../assets/MCP_INSTRUCTIONS.md");

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

/// Metadata about the database file at the time the store was opened
#[derive(Debug, Clone)]
struct DbFileMetadata {
    /// File size in bytes
    size: u64,
    /// File modification time
    mtime: std::time::SystemTime,
}

impl DbFileMetadata {
    /// Capture current metadata for a database file
    fn capture(db_path: &Path) -> Option<Self> {
        let metadata = fs::metadata(db_path).ok()?;
        Some(Self {
            size: metadata.len(),
            mtime: metadata.modified().ok()?,
        })
    }

    /// Check if the current file matches this metadata
    fn matches(&self, db_path: &Path) -> bool {
        if let Some(current) = Self::capture(db_path) {
            self.size == current.size && self.mtime == current.mtime
        } else {
            false // File doesn't exist anymore
        }
    }
}

/// Cached workspace information
struct WorkspaceInfo {
    root: PathBuf,
    db_path: PathBuf,
    store: Option<IndexStore>,
    /// Metadata of the DB file when the store was opened (for staleness detection)
    db_metadata: Option<DbFileMetadata>,
    last_used: std::time::Instant,
}

/// MCP Server state with multi-workspace support
pub struct McpServer {
    /// Default workspace (from --workspace flag), used when no file path is provided
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
        // Try to ensure default workspace index is available (auto-start daemon if needed).
        // If this fails, we still proceed with initialization - tools can retry on first call.
        // This makes the MCP server more resilient to transient startup issues.
        let default_workspace = self.default_workspace.clone();
        if let Err(e) = self.ensure_workspace_index(&default_workspace) {
            log::warn!(
                "Could not initialize index during startup: {}. Will retry on first tool call.",
                e
            );
            // Don't fail - let initialization succeed so tools are available
            // Tools will retry ensure_workspace_index when called
        }

        self.initialized = true;

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
            },
            "instructions": SERVER_INSTRUCTIONS
        }))
    }

    fn handle_tools_list(&self) -> Result<Value> {
        let tools = vec![Tool {
            name: "gabb_structure".to_string(),
            description: concat!(
                "Get a CHEAP, LIGHTWEIGHT overview of a file's symbols before reading it.\n\n",
                "⚠️ MANDATORY PRE-READ CHECK: Before calling Read on any .py/.ts/.tsx/.rs/.kt/.cpp/.cc/.hpp file, ",
                "you MUST call gabb_structure FIRST. Reading a large file directly can cost 5,000-10,000 tokens. ",
                "gabb_structure costs ~50 tokens and shows you what's inside, so you can then Read with offset/limit.\n\n",
                "The ONLY exceptions are: (1) files <50 lines where direct Read is fine, ",
                "(2) files you've already seen structure for in this conversation.\n\n",
                "Returns: symbol names, kinds, line numbers—NOT source code. ",
                "After seeing structure, use targeted Read with offset/limit."
            )
            .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Path to the file to analyze"
                    }
                },
                "required": ["file"]
            }),
        }];

        Ok(json!({ "tools": tools }))
    }

    fn handle_tools_call(&mut self, params: &Value) -> Result<Value> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;

        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let result = match name {
            "gabb_structure" => self.tool_structure(&arguments),
            _ => Ok(ToolResult::error(format!("Unknown tool: {}", name))),
        };

        // Handle database corruption errors with automatic recovery
        let result = match result {
            Err(ref e) if Self::is_database_corruption_error(e) => {
                // Check if version mismatch might be the cause
                let version_info = self.check_version_mismatch_info();

                log::warn!(
                    "Database corruption detected: {}. {}Triggering automatic rebuild.",
                    e,
                    if version_info.is_some() {
                        "Version mismatch detected. "
                    } else {
                        ""
                    }
                );

                // Invalidate all cached workspaces to force rebuild
                let workspaces = self.cached_workspace_roots();
                for workspace in workspaces {
                    self.invalidate_workspace(&workspace);
                }

                // Return a message for the AI agent - include version mismatch info if relevant
                let message = if let Some(info) = version_info {
                    format!(
                        "Index corruption detected (likely caused by version mismatch: {}). \
                         Automatically resolved. Retry this query.",
                        info
                    )
                } else {
                    "Index corruption detected and automatically resolved. \
                     Retry this query - the index will rebuild automatically."
                        .to_string()
                };
                Ok(ToolResult::text(message))
            }
            other => other,
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

        // Use the shared workspace discovery logic
        workspace::find_workspace_root_from(&file_path)
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
                    db_metadata: None,
                    last_used: std::time::Instant::now(),
                },
            );
        }

        // Update last_used and return
        let info = self.workspace_cache.get_mut(&workspace_root).unwrap();
        info.last_used = std::time::Instant::now();
        Ok(info)
    }

    /// Ensure index exists for a workspace, starting daemon if needed.
    /// Uses the shared daemon logic that handles:
    /// - Auto-starting daemon if index doesn't exist
    /// - Waiting for database to be fully ready (not just file existence)
    /// - Schema version mismatches (auto-rebuild)
    /// - Corrupt databases (auto-rebuild)
    fn ensure_workspace_index(&mut self, workspace_root: &Path) -> Result<()> {
        use crate::daemon;

        let info = self.get_or_create_workspace(workspace_root)?;

        if info.store.is_some() {
            return Ok(());
        }

        // Clone paths before the borrow ends
        let root = info.root.clone();
        let db_path = info.db_path.clone();

        // Use the shared daemon logic for proper index initialization
        let opts = daemon::EnsureIndexOptions {
            no_start_daemon: false,
            timeout: std::time::Duration::from_secs(60),
            no_daemon_warnings: true, // Suppress warnings in MCP context
            auto_restart_on_version_mismatch: true, // Auto-restart daemon when version differs
        };

        daemon::ensure_index_available(&root, &db_path, &opts)?;

        // Re-get info (borrow checker) and open the store
        let info = self.workspace_cache.get_mut(workspace_root).unwrap();
        info.store = Some(IndexStore::open(&info.db_path)?);
        // Capture DB metadata for staleness detection
        info.db_metadata = DbFileMetadata::capture(&info.db_path);
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

    /// Check if a cached store is stale (DB file changed or missing)
    fn is_store_stale(&self, workspace_root: &Path) -> bool {
        if let Some(info) = self.workspace_cache.get(workspace_root) {
            // No store cached = not stale (needs initialization)
            if info.store.is_none() {
                return false;
            }

            // Check if DB file still exists
            if !info.db_path.exists() {
                log::info!("Database file no longer exists: {}", info.db_path.display());
                return true;
            }

            // Check if DB metadata changed (file was modified/rebuilt)
            if let Some(ref cached_metadata) = info.db_metadata {
                if !cached_metadata.matches(&info.db_path) {
                    log::info!(
                        "Database file changed since last access: {}",
                        info.db_path.display()
                    );
                    return true;
                }
            }
        }
        false
    }

    /// Invalidate a stale cached store, forcing reconnection on next access
    fn invalidate_stale_store(&mut self, workspace_root: &Path) {
        if let Some(info) = self.workspace_cache.get_mut(workspace_root) {
            log::info!(
                "Invalidating stale store for workspace: {}",
                workspace_root.display()
            );
            info.store = None;
            info.db_metadata = None;
        }
    }

    /// Get store for a workspace (ensures index exists and is fresh)
    fn get_store_for_workspace(&mut self, workspace_root: &Path) -> Result<&IndexStore> {
        // Check if cached store is stale before using it
        if self.is_store_stale(workspace_root) {
            self.invalidate_stale_store(workspace_root);
        }

        self.ensure_workspace_index(workspace_root)?;
        let info = self.workspace_cache.get(workspace_root).unwrap();
        info.store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Store not initialized"))
    }

    /// Check if an error indicates database corruption
    fn is_database_corruption_error(err: &anyhow::Error) -> bool {
        let err_str = err.to_string().to_lowercase();
        err_str.contains("malformed")
            || err_str.contains("corrupt")
            || err_str.contains("disk i/o error")
            || err_str.contains("database disk image")
            || err_str.contains("file is not a database")
            || err_str.contains("sqlite_corrupt")
            || err_str.contains("sqlite_notadb")
    }

    /// Check if there's a version mismatch between MCP server and any running daemon
    /// Returns a description of the mismatch if found
    fn check_version_mismatch_info(&self) -> Option<String> {
        use crate::daemon;

        let mcp_version = SERVER_VERSION;

        // Check default workspace first
        if let Ok(Some(pid_info)) = daemon::read_pid_file(&self.default_workspace) {
            if daemon::is_process_running(pid_info.pid) && pid_info.version != mcp_version {
                return Some(format!(
                    "daemon v{} vs MCP server v{}",
                    pid_info.version, mcp_version
                ));
            }
        }

        // Check cached workspaces
        for workspace_root in self.workspace_cache.keys() {
            if let Ok(Some(pid_info)) = daemon::read_pid_file(workspace_root) {
                if daemon::is_process_running(pid_info.pid) && pid_info.version != mcp_version {
                    return Some(format!(
                        "daemon v{} vs MCP server v{}",
                        pid_info.version, mcp_version
                    ));
                }
            }
        }

        None
    }

    /// Invalidate cached store for a workspace, forcing rebuild on next access
    fn invalidate_workspace(&mut self, workspace_root: &Path) {
        if let Some(info) = self.workspace_cache.get_mut(workspace_root) {
            log::info!(
                "Invalidating cached store for workspace: {}",
                workspace_root.display()
            );
            // Drop the store and metadata
            info.store = None;
            info.db_metadata = None;

            // Delete the database files to force rebuild
            let db_path = &info.db_path;
            let _ = std::fs::remove_file(db_path);
            let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
            let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
        }
    }

    /// Get all workspace roots that are currently cached
    fn cached_workspace_roots(&self) -> Vec<PathBuf> {
        self.workspace_cache.keys().cloned().collect()
    }

    // ==================== Tool Implementations ====================

    fn tool_symbols(&mut self, args: &Value) -> Result<ToolResult> {
        let name = args.get("name").and_then(|v| v.as_str());
        let name_pattern = args.get("name_pattern").and_then(|v| v.as_str());
        let name_contains = args.get("name_contains").and_then(|v| v.as_str());
        let name_fts = args.get("name_fts").and_then(|v| v.as_str());
        let case_insensitive = args
            .get("case_insensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let kind = args.get("kind").and_then(|v| v.as_str());
        let file = args.get("file").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
        let namespace = args.get("namespace").and_then(|v| v.as_str());
        let scope = args.get("scope").and_then(|v| v.as_str());

        // Format options
        let include_source = args
            .get("include_source")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let context_lines = args
            .get("context_lines")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        // Infer workspace from file path if provided
        let workspace = self.workspace_for_file(file);
        let store = self.get_store_for_workspace(&workspace)?;

        // Extract pagination parameters
        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let after = args.get("after").and_then(|v| v.as_str());

        let query = SymbolQuery {
            file,
            kind,
            name,
            name_pattern,
            name_contains,
            name_fts,
            case_insensitive,
            limit: Some(limit),
            offset,
            after,
            namespace,
            scope,
        };

        let symbols = store.list_symbols_filtered(&query)?;

        if symbols.is_empty() {
            return Ok(ToolResult::text("No symbols found matching the criteria."));
        }

        let format_opts = FormatOptions {
            include_source,
            context_lines,
        };

        let output = format_symbols(&symbols, &workspace, &format_opts);
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

        let output = format_symbols(&symbols, &workspace, &FormatOptions::default());
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

        // Format options - default to including source for definitions
        let include_source = args
            .get("include_source")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let context_lines = args
            .get("context_lines")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let format_opts = FormatOptions {
            include_source,
            context_lines,
        };

        // Infer workspace from file path
        let workspace = self.workspace_for_file(Some(file));
        let file_path = self.resolve_path_for_workspace(file, &workspace);

        if !file_path.exists() {
            return Ok(ToolResult::error(format!(
                "File not found: {}",
                file_path.display()
            )));
        }

        // Convert line:col to byte offset
        let contents = std::fs::read(&file_path)?;
        let offset = line_col_to_offset(&contents, line, character)
            .ok_or_else(|| anyhow::anyhow!("Could not map line/character to byte offset"))?
            as i64;

        let file_str = crate::store::normalize_path(&file_path);
        let store = self.get_store_for_workspace(&workspace)?;

        // First, check if cursor is on a recorded reference - if so, look up its target symbol
        let definition = if let Some(ref_record) = store.reference_at_position(&file_str, offset)? {
            // Found a reference - look up the symbol it points to
            let symbols = store.symbols_by_ids(std::slice::from_ref(&ref_record.symbol_id))?;
            if let Some(sym) = symbols.into_iter().next() {
                Some(sym)
            } else {
                // Reference exists but symbol not found - fall back to find_symbol_at
                self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)?
            }
        } else {
            // No reference at position - find symbol at position directly
            self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)?
        };

        if let Some(symbol) = definition {
            let output = format_symbol(&symbol, &workspace, &format_opts);
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
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        let include_definition = args
            .get("include_definition")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        // Infer workspace from file path
        let workspace = self.workspace_for_file(Some(file));
        let file_path = self.resolve_path_for_workspace(file, &workspace);

        // Find symbol at position
        let symbol =
            match self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)? {
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

        if refs.is_empty() && format != "refactor" {
            return Ok(ToolResult::text(format!(
                "No usages found for '{}'",
                symbol.name
            )));
        }

        let refs: Vec<_> = refs.into_iter().take(limit).collect();

        // Refactor format - returns JSON structured for Edit tool
        if format == "refactor" {
            return self.format_usages_for_refactor(&symbol, &refs, &workspace, include_definition);
        }

        // Default format
        let mut output = format!("Usages of '{}' ({} found):\n\n", symbol.name, refs.len());
        for r in &refs {
            let rel_path = relative_path_for_workspace(&r.file, &workspace);
            // Convert offset to line:col
            if let Ok((ref_line, ref_col)) = offset_to_line_col_in_file(&r.file, r.start as usize) {
                output.push_str(&format!("  {}:{}:{}\n", rel_path, ref_line, ref_col));
            }
        }

        Ok(ToolResult::text(output))
    }

    /// Format usages for refactoring (rename operations)
    fn format_usages_for_refactor(
        &self,
        symbol: &SymbolRecord,
        refs: &[crate::store::ReferenceRecord],
        workspace: &Path,
        include_definition: bool,
    ) -> Result<ToolResult> {
        let mut edits: Vec<Value> = Vec::new();

        // Include the definition location first if requested
        if include_definition {
            if let Ok((def_line, def_col)) =
                offset_to_line_col_in_file(&symbol.file, symbol.start as usize)
            {
                if let Ok((def_end_line, def_end_col)) =
                    offset_to_line_col_in_file(&symbol.file, symbol.end as usize)
                {
                    let rel_path = relative_path_for_workspace(&symbol.file, workspace);
                    let context =
                        get_line_at_offset(&symbol.file, symbol.start as usize).unwrap_or_default();

                    edits.push(json!({
                        "file": rel_path,
                        "line": def_line,
                        "column": def_col,
                        "end_line": def_end_line,
                        "end_column": def_end_col,
                        "old_text": &symbol.name,
                        "context": context,
                        "is_definition": true
                    }));
                }
            }
        }

        // Add all usages
        for r in refs {
            let rel_path = relative_path_for_workspace(&r.file, workspace);

            // Get start position
            let (ref_line, ref_col) = match offset_to_line_col_in_file(&r.file, r.start as usize) {
                Ok(pos) => pos,
                Err(_) => continue,
            };

            // Get end position
            let (end_line, end_col) = match offset_to_line_col_in_file(&r.file, r.end as usize) {
                Ok(pos) => pos,
                Err(_) => continue,
            };

            // Extract the actual text at this location
            let old_text = extract_text_at_offset(&r.file, r.start as usize, r.end as usize)
                .unwrap_or_else(|| symbol.name.clone());

            // Get context line
            let context = get_line_at_offset(&r.file, r.start as usize).unwrap_or_default();

            edits.push(json!({
                "file": rel_path,
                "line": ref_line,
                "column": ref_col,
                "end_line": end_line,
                "end_column": end_col,
                "old_text": old_text,
                "context": context,
                "is_definition": false
            }));
        }

        // Get definition location for symbol info
        let (def_line, def_col) =
            offset_to_line_col_in_file(&symbol.file, symbol.start as usize).unwrap_or((0, 0));
        let def_rel_path = relative_path_for_workspace(&symbol.file, workspace);

        let output = json!({
            "symbol": {
                "name": symbol.name,
                "kind": symbol.kind,
                "definition_file": def_rel_path,
                "definition_line": def_line,
                "definition_column": def_col
            },
            "edits": edits,
            "total_count": edits.len()
        });

        Ok(ToolResult::text(
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string()),
        ))
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
        let symbol =
            match self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)? {
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
            let output = format_symbols(&fallback, &workspace, &FormatOptions::default());
            return Ok(ToolResult::text(format!(
                "Implementations of '{}' (by name):\n\n{}",
                symbol.name, output
            )));
        }

        impls.truncate(limit);
        let output = format_symbols(&impls, &workspace, &FormatOptions::default());
        Ok(ToolResult::text(format!(
            "Implementations of '{}':\n\n{}",
            symbol.name, output
        )))
    }

    fn tool_daemon_status(&mut self) -> Result<ToolResult> {
        use crate::daemon;

        let mut status = String::new();
        let mcp_version = SERVER_VERSION;

        // Show default workspace status
        status.push_str("Default Workspace:\n");
        if let Ok(Some(pid_info)) = daemon::read_pid_file(&self.default_workspace) {
            if daemon::is_process_running(pid_info.pid) {
                // Check for version mismatch
                let version_warning = if pid_info.version != mcp_version {
                    format!(
                        "\n  ⚠️  VERSION MISMATCH: daemon={}, MCP server={}\n  \
                         This can cause corruption errors. Run: gabb daemon restart\n",
                        pid_info.version, mcp_version
                    )
                } else {
                    String::new()
                };

                status.push_str(&format!(
                    "  Daemon: running (PID {})\n  Version: {}\n  MCP Server: {}\n  Root: {}\n  Database: {}\n{}",
                    pid_info.pid,
                    pid_info.version,
                    mcp_version,
                    self.default_workspace.display(),
                    self.default_db_path.display(),
                    version_warning
                ));
                // Add index stats if available
                if let Ok(store) = self.get_store_for_workspace(&self.default_workspace.clone()) {
                    if let Ok(stats) = store.get_index_stats() {
                        status.push_str(&format!(
                            "  Index: {} files, {} symbols\n",
                            stats.files.total, stats.symbols.total
                        ));
                        if let Some(ref last_time) = stats.index.last_updated {
                            status.push_str(&format!("  Last indexed: {}\n", last_time));
                        }
                    }
                }
                status.push_str("  Activity: watching for changes\n");
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
            // Show stats even if daemon is not running
            if self.default_db_path.exists() {
                if let Ok(store) = self.get_store_for_workspace(&self.default_workspace.clone()) {
                    if let Ok(stats) = store.get_index_stats() {
                        status.push_str(&format!(
                            "  Index: {} files, {} symbols\n",
                            stats.files.total, stats.symbols.total
                        ));
                        if let Some(ref last_time) = stats.index.last_updated {
                            status.push_str(&format!("  Last indexed: {}\n", last_time));
                        }
                    }
                }
            }
        }

        // Show cached workspaces
        if !self.workspace_cache.is_empty() {
            status.push_str(&format!(
                "\nCached Workspaces ({}/{}):\n",
                self.workspace_cache.len(),
                MAX_CACHED_WORKSPACES
            ));
            for (root, info) in &self.workspace_cache {
                let (daemon_status, version_warning) =
                    if let Ok(Some(pid_info)) = daemon::read_pid_file(root) {
                        if daemon::is_process_running(pid_info.pid) {
                            let warning = if pid_info.version != mcp_version {
                                " ⚠️ VERSION MISMATCH"
                            } else {
                                ""
                            };
                            (
                                format!("running (PID {}, v{})", pid_info.pid, pid_info.version),
                                warning,
                            )
                        } else {
                            ("not running".to_string(), "")
                        }
                    } else {
                        ("not running".to_string(), "")
                    };
                let index_status = if info.store.is_some() {
                    "loaded"
                } else if info.db_path.exists() {
                    "available"
                } else {
                    "not indexed"
                };
                status.push_str(&format!(
                    "  {}\n    Daemon: {}, Index: {}{}\n",
                    root.display(),
                    daemon_status,
                    index_status,
                    version_warning
                ));
            }
        }

        Ok(ToolResult::text(status))
    }

    fn tool_stats(&mut self) -> Result<ToolResult> {
        // Use default workspace
        let workspace = self.default_workspace.clone();
        let store = self.get_store_for_workspace(&workspace)?;

        let stats = store.get_index_stats()?;

        // Format as JSON for structured output
        Ok(ToolResult::text(serde_json::to_string_pretty(&stats)?))
    }

    fn tool_duplicates(&mut self, args: &Value) -> Result<ToolResult> {
        let kind = args.get("kind").and_then(|v| v.as_str());
        let min_count = args.get("min_count").and_then(|v| v.as_u64()).unwrap_or(2) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

        // Use default workspace
        let workspace = self.default_workspace.clone();
        let store = self.get_store_for_workspace(&workspace)?;

        let groups = store.find_duplicate_groups(min_count, kind, None)?;

        if groups.is_empty() {
            return Ok(ToolResult::text("No duplicate code found."));
        }

        let groups: Vec<_> = groups.into_iter().take(limit).collect();
        let output = format_duplicate_groups(&groups, &workspace);
        Ok(ToolResult::text(output))
    }

    fn tool_includers(&mut self, args: &Value) -> Result<ToolResult> {
        let file = args
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'file' argument"))?;
        let transitive = args
            .get("transitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        // Infer workspace from file path
        let file_path = PathBuf::from(file);
        let workspace = self
            .infer_workspace(&file_path)
            .unwrap_or_else(|| self.default_workspace.clone());
        let full_path = self.resolve_path_for_workspace(file, &workspace);

        let store = self.get_store_for_workspace(&workspace)?;
        let file_str = crate::store::normalize_path(&full_path);

        let mut files: Vec<String> = if transitive {
            store.get_invalidation_set(&file_str)?
        } else {
            store.get_dependents(&file_str)?
        };

        // Remove the original file from results
        files.retain(|f| f != &file_str);

        let files: Vec<_> = files.into_iter().take(limit).collect();

        if files.is_empty() {
            let transitive_str = if transitive { " (transitive)" } else { "" };
            return Ok(ToolResult::text(format!(
                "No files found that include {}{}",
                file, transitive_str
            )));
        }

        let output = format_file_list(&files, &workspace, file, "includers", transitive);
        Ok(ToolResult::text(output))
    }

    fn tool_includes(&mut self, args: &Value) -> Result<ToolResult> {
        let file = args
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'file' argument"))?;
        let transitive = args
            .get("transitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        // Infer workspace from file path
        let file_path = PathBuf::from(file);
        let workspace = self
            .infer_workspace(&file_path)
            .unwrap_or_else(|| self.default_workspace.clone());
        let full_path = self.resolve_path_for_workspace(file, &workspace);

        let store = self.get_store_for_workspace(&workspace)?;
        let file_str = crate::store::normalize_path(&full_path);

        let files: Vec<String> = if transitive {
            store.get_transitive_dependencies(&file_str)?
        } else {
            store
                .get_file_dependencies(&file_str)?
                .into_iter()
                .map(|d| d.to_file)
                .collect()
        };

        let files: Vec<_> = files.into_iter().take(limit).collect();

        if files.is_empty() {
            let transitive_str = if transitive { " (transitive)" } else { "" };
            return Ok(ToolResult::text(format!(
                "No includes found for {}{}",
                file, transitive_str
            )));
        }

        let output = format_file_list(&files, &workspace, file, "includes", transitive);
        Ok(ToolResult::text(output))
    }

    fn tool_structure(&mut self, args: &Value) -> Result<ToolResult> {
        let file = args
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'file' argument"))?;

        // Infer workspace from file path
        let file_path = PathBuf::from(file);
        let workspace = self
            .infer_workspace(&file_path)
            .unwrap_or_else(|| self.default_workspace.clone());
        let full_path = self.resolve_path_for_workspace(file, &workspace);

        if !full_path.exists() {
            return Ok(ToolResult::error(format!(
                "File not found: {}",
                full_path.display()
            )));
        }

        let store = self.get_store_for_workspace(&workspace)?;
        let file_str = crate::store::normalize_path(&full_path);

        // Query symbols for this file
        let query = SymbolQuery {
            file: Some(&file_str),
            ..Default::default()
        };
        let symbols = store.list_symbols_filtered(&query)?;

        if symbols.is_empty() {
            return Ok(ToolResult::error(format!(
                "No symbols found in {}. Is it indexed?",
                file_str
            )));
        }

        // Compute summary (counts by kind, line count, key types)
        let summary = compute_file_summary(&symbols, full_path.to_str().unwrap_or(&file_str));
        let summary_text = format_file_summary(&summary);

        // Build hierarchical structure and format as compact text
        let tree = build_structure_tree(&symbols, &file_str)?;
        let mut output = format!("{}:{}\n{}\n", file_str, summary.line_count, summary_text);
        format_structure_tree(&tree, 0, &mut output);

        Ok(ToolResult::text(output))
    }

    fn tool_supertypes(&mut self, args: &Value) -> Result<ToolResult> {
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
        let transitive = args
            .get("transitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let include_source = args
            .get("include_source")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        // Infer workspace from file path
        let workspace = self.workspace_for_file(Some(file));
        let file_path = self.resolve_path_for_workspace(file, &workspace);

        // Find symbol at position
        let symbol =
            match self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)? {
                Some(s) => s,
                None => {
                    return Ok(ToolResult::text(format!(
                        "No symbol found at {}:{}:{}",
                        file, line, character
                    )));
                }
            };

        // Find supertypes
        let store = self.get_store_for_workspace(&workspace)?;
        let mut supertypes = store.supertypes(&symbol.id, transitive)?;

        if supertypes.is_empty() {
            return Ok(ToolResult::text(format!(
                "No supertypes found for '{}'",
                symbol.name
            )));
        }

        supertypes.truncate(limit);

        let format_opts = FormatOptions {
            include_source,
            context_lines: None,
        };
        let output = format_symbols(&supertypes, &workspace, &format_opts);

        let label = if transitive {
            "Supertypes (transitive)"
        } else {
            "Supertypes"
        };
        Ok(ToolResult::text(format!(
            "{} of '{}' ({} found):\n\n{}",
            label,
            symbol.name,
            supertypes.len(),
            output
        )))
    }

    fn tool_subtypes(&mut self, args: &Value) -> Result<ToolResult> {
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
        let transitive = args
            .get("transitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let include_source = args
            .get("include_source")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        // Infer workspace from file path
        let workspace = self.workspace_for_file(Some(file));
        let file_path = self.resolve_path_for_workspace(file, &workspace);

        // Find symbol at position
        let symbol =
            match self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)? {
                Some(s) => s,
                None => {
                    return Ok(ToolResult::text(format!(
                        "No symbol found at {}:{}:{}",
                        file, line, character
                    )));
                }
            };

        // Find subtypes
        let store = self.get_store_for_workspace(&workspace)?;
        let mut subtypes = store.subtypes(&symbol.id, transitive)?;

        if subtypes.is_empty() {
            return Ok(ToolResult::text(format!(
                "No subtypes found for '{}'",
                symbol.name
            )));
        }

        subtypes.truncate(limit);

        let format_opts = FormatOptions {
            include_source,
            context_lines: None,
        };
        let output = format_symbols(&subtypes, &workspace, &format_opts);

        let label = if transitive {
            "Subtypes (transitive)"
        } else {
            "Subtypes"
        };
        Ok(ToolResult::text(format!(
            "{} of '{}' ({} found):\n\n{}",
            label,
            symbol.name,
            subtypes.len(),
            output
        )))
    }

    fn tool_rename(&mut self, args: &Value) -> Result<ToolResult> {
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
        let new_name = args
            .get("new_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'new_name' argument"))?;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;

        // Infer workspace from file path
        let workspace = self.workspace_for_file(Some(file));
        let file_path = self.resolve_path_for_workspace(file, &workspace);

        // Find symbol at position
        let symbol =
            match self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)? {
                Some(s) => s,
                None => {
                    return Ok(ToolResult::text(format!(
                        "No symbol found at {}:{}:{}",
                        file, line, character
                    )));
                }
            };

        // Find references
        let store = self.get_store_for_workspace(&workspace)?;
        let refs = store.references_for_symbol(&symbol.id)?;
        let refs: Vec<_> = refs.into_iter().take(limit.saturating_sub(1)).collect();

        let mut edits: Vec<Value> = Vec::new();

        // Include the definition location first
        if let Ok((def_line, def_col)) =
            offset_to_line_col_in_file(&symbol.file, symbol.start as usize)
        {
            if let Ok((def_end_line, def_end_col)) =
                offset_to_line_col_in_file(&symbol.file, symbol.end as usize)
            {
                let rel_path = relative_path_for_workspace(&symbol.file, &workspace);
                let context =
                    get_line_at_offset(&symbol.file, symbol.start as usize).unwrap_or_default();

                edits.push(json!({
                    "file": rel_path,
                    "line": def_line,
                    "column": def_col,
                    "end_line": def_end_line,
                    "end_column": def_end_col,
                    "old_text": &symbol.name,
                    "new_text": new_name,
                    "context": context,
                    "is_definition": true
                }));
            }
        }

        // Add all usages
        for r in &refs {
            let rel_path = relative_path_for_workspace(&r.file, &workspace);

            let (ref_line, ref_col) = match offset_to_line_col_in_file(&r.file, r.start as usize) {
                Ok(pos) => pos,
                Err(_) => continue,
            };

            let (end_line, end_col) = match offset_to_line_col_in_file(&r.file, r.end as usize) {
                Ok(pos) => pos,
                Err(_) => continue,
            };

            let old_text = extract_text_at_offset(&r.file, r.start as usize, r.end as usize)
                .unwrap_or_else(|| symbol.name.clone());

            let context = get_line_at_offset(&r.file, r.start as usize).unwrap_or_default();

            edits.push(json!({
                "file": rel_path,
                "line": ref_line,
                "column": ref_col,
                "end_line": end_line,
                "end_column": end_col,
                "old_text": old_text,
                "new_text": new_name,
                "context": context,
                "is_definition": false
            }));
        }

        // Get definition location for symbol info
        let (def_line, def_col) =
            offset_to_line_col_in_file(&symbol.file, symbol.start as usize).unwrap_or((0, 0));
        let def_rel_path = relative_path_for_workspace(&symbol.file, &workspace);

        let output = json!({
            "rename": {
                "from": symbol.name,
                "to": new_name
            },
            "symbol": {
                "name": symbol.name,
                "kind": symbol.kind,
                "definition_file": def_rel_path,
                "definition_line": def_line,
                "definition_column": def_col
            },
            "edits": edits,
            "total_count": edits.len(),
            "definition_included": true
        });

        Ok(ToolResult::text(
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string()),
        ))
    }

    fn tool_callers(&mut self, args: &Value) -> Result<ToolResult> {
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
        let transitive = args
            .get("transitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let include_source = args
            .get("include_source")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        // Infer workspace from file path
        let workspace = self.workspace_for_file(Some(file));
        let file_path = self.resolve_path_for_workspace(file, &workspace);

        // Find symbol at position
        let symbol =
            match self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)? {
                Some(s) => s,
                None => {
                    return Ok(ToolResult::text(format!(
                        "No symbol found at {}:{}:{}",
                        file, line, character
                    )));
                }
            };

        // Find callers
        let store = self.get_store_for_workspace(&workspace)?;
        let mut callers = store.callers(&symbol.id, transitive)?;

        if callers.is_empty() {
            return Ok(ToolResult::text(format!(
                "No callers found for '{}'",
                symbol.name
            )));
        }

        callers.truncate(limit);

        let format_opts = FormatOptions {
            include_source,
            context_lines: None,
        };
        let output = format_symbols(&callers, &workspace, &format_opts);

        let label = if transitive {
            "Callers (transitive)"
        } else {
            "Callers"
        };
        Ok(ToolResult::text(format!(
            "{} of '{}' ({} found):\n\n{}",
            label,
            symbol.name,
            callers.len(),
            output
        )))
    }

    fn tool_callees(&mut self, args: &Value) -> Result<ToolResult> {
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
        let transitive = args
            .get("transitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let include_source = args
            .get("include_source")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        // Infer workspace from file path
        let workspace = self.workspace_for_file(Some(file));
        let file_path = self.resolve_path_for_workspace(file, &workspace);

        // Find symbol at position
        let symbol =
            match self.find_symbol_at_in_workspace(&file_path, line, character, &workspace)? {
                Some(s) => s,
                None => {
                    return Ok(ToolResult::text(format!(
                        "No symbol found at {}:{}:{}",
                        file, line, character
                    )));
                }
            };

        // Find callees
        let store = self.get_store_for_workspace(&workspace)?;
        let mut callees = store.callees(&symbol.id, transitive)?;

        if callees.is_empty() {
            return Ok(ToolResult::text(format!(
                "No callees found for '{}'",
                symbol.name
            )));
        }

        callees.truncate(limit);

        let format_opts = FormatOptions {
            include_source,
            context_lines: None,
        };
        let output = format_symbols(&callees, &workspace, &format_opts);

        let label = if transitive {
            "Callees (transitive)"
        } else {
            "Callees"
        };
        Ok(ToolResult::text(format!(
            "{} of '{}' ({} found):\n\n{}",
            label,
            symbol.name,
            callees.len(),
            output
        )))
    }

    // ==================== Helper Methods ====================

    /// Resolve a file path to an absolute path.
    ///
    /// Relative paths are always resolved against `default_workspace` (the CWD),
    /// NOT the inferred workspace. This is because user-provided relative paths
    /// are relative to their current directory, not to whatever workspace root
    /// gabb discovers.
    ///
    /// The `_workspace` parameter is kept for API consistency but is unused -
    /// the workspace is only used for determining which store to query, not
    /// for path resolution.
    fn resolve_path_for_workspace(&self, path: &str, _workspace: &Path) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            self.default_workspace.join(p)
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

fn format_symbols(symbols: &[SymbolRecord], workspace_root: &Path, opts: &FormatOptions) -> String {
    let mut output = String::new();
    for sym in symbols {
        output.push_str(&format_symbol(sym, workspace_root, opts));
        output.push('\n');
    }
    output
}

fn format_symbol(sym: &SymbolRecord, workspace_root: &Path, opts: &FormatOptions) -> String {
    let rel_path = PathBuf::from(&sym.file)
        .strip_prefix(workspace_root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| sym.file.clone());

    // Determine context from file path OR inline test markers (#[cfg(test)], #[test])
    let context = if sym.is_test || is_test_file(&sym.file) {
        "test"
    } else {
        "prod"
    };

    // Convert byte offset to line:col
    let location =
        if let Ok((line, col)) = offset_to_line_col_in_file(&sym.file, sym.start as usize) {
            format!("{}:{}:{}", rel_path, line, col)
        } else {
            format!("{}:offset:{}", rel_path, sym.start)
        };

    let mut parts = vec![format!(
        "{:<10} {:<30} [{}] {}",
        sym.kind, sym.name, context, location
    )];

    if let Some(ref vis) = sym.visibility {
        parts.push(format!("  visibility: {}", vis));
    }
    if let Some(ref container) = sym.container {
        parts.push(format!("  container: {}", container));
    }

    // Include source code if requested
    if opts.include_source {
        if let Some(source) = extract_source(&sym.file, sym.start, sym.end, opts.context_lines) {
            parts.push("  source: |".to_string());
            for line in source.lines() {
                parts.push(format!("    {}", line));
            }
        }
    }

    parts.join("\n")
}

/// Extract source code from a file using byte offsets, optionally with context lines
pub fn extract_source(
    file_path: &str,
    start_byte: i64,
    end_byte: i64,
    context_lines: Option<usize>,
) -> Option<String> {
    let content = fs::read_to_string(file_path).ok()?;
    let bytes = content.as_bytes();

    let start = start_byte as usize;
    let end = end_byte as usize;

    if start >= bytes.len() || end > bytes.len() || start >= end {
        return None;
    }

    // If no context lines, just return the symbol source
    if context_lines.is_none() || context_lines == Some(0) {
        return Some(String::from_utf8_lossy(&bytes[start..end]).to_string());
    }

    let context = context_lines.unwrap();

    // Find line boundaries for context
    let mut line_starts: Vec<usize> = vec![0];
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' && i + 1 < bytes.len() {
            line_starts.push(i + 1);
        }
    }

    // Find which line the symbol starts on
    let symbol_start_line = line_starts
        .iter()
        .position(|&pos| pos > start)
        .unwrap_or(line_starts.len())
        .saturating_sub(1);

    // Find which line the symbol ends on
    let symbol_end_line = line_starts
        .iter()
        .position(|&pos| pos > end)
        .unwrap_or(line_starts.len())
        .saturating_sub(1);

    // Calculate context range
    let context_start_line = symbol_start_line.saturating_sub(context);
    let context_end_line = (symbol_end_line + context + 1).min(line_starts.len());

    // Get the byte range for context
    let context_start_byte = line_starts[context_start_line];
    let context_end_byte = if context_end_line >= line_starts.len() {
        bytes.len()
    } else {
        line_starts
            .get(context_end_line)
            .copied()
            .unwrap_or(bytes.len())
    };

    Some(String::from_utf8_lossy(&bytes[context_start_byte..context_end_byte]).to_string())
}

fn format_duplicate_groups(groups: &[DuplicateGroup], workspace_root: &Path) -> String {
    let total_groups = groups.len();
    let total_duplicates: usize = groups.iter().map(|g| g.symbols.len()).sum();

    let mut output = format!(
        "Found {} duplicate groups ({} total symbols)\n\n",
        total_groups, total_duplicates
    );

    for (i, group) in groups.iter().enumerate() {
        let short_hash = &group.content_hash[..8.min(group.content_hash.len())];
        output.push_str(&format!(
            "Group {} ({} duplicates, hash: {}):\n",
            i + 1,
            group.symbols.len(),
            short_hash
        ));

        for sym in &group.symbols {
            let rel_path = PathBuf::from(&sym.file)
                .strip_prefix(workspace_root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| sym.file.clone());

            let location = if let Ok((line, col)) =
                offset_to_line_col_in_file(&sym.file, sym.start as usize)
            {
                format!("{}:{}:{}", rel_path, line, col)
            } else {
                format!("{}:offset:{}", rel_path, sym.start)
            };

            let container = sym
                .container
                .as_ref()
                .map(|c| format!(" in {}", c))
                .unwrap_or_default();

            output.push_str(&format!(
                "  {:<10} {:<30} {}{}\n",
                sym.kind, sym.name, location, container
            ));
        }
        output.push('\n');
    }

    output
}

fn format_file_list(
    files: &[String],
    workspace_root: &Path,
    source_file: &str,
    relation: &str,
    transitive: bool,
) -> String {
    let transitive_str = if transitive { " (transitive)" } else { "" };
    let mut output = format!(
        "Found {} {}{} for {}:\n\n",
        files.len(),
        relation,
        transitive_str,
        source_file
    );

    for file in files {
        let rel_path = PathBuf::from(file)
            .strip_prefix(workspace_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| file.clone());
        output.push_str(&format!("  {}\n", rel_path));
    }

    output
}

/// Convert byte offset to 1-based line:column for a file
fn offset_to_line_col_in_file(file_path: &str, offset: usize) -> Result<(usize, usize)> {
    crate::offset_to_line_col_in_file(std::path::Path::new(file_path), offset)
        .map_err(|e| anyhow::anyhow!("{}", e))
}

/// Convert 1-based line:column to byte offset
fn line_col_to_offset(buf: &[u8], line: usize, character: usize) -> Option<usize> {
    if line == 0 || character == 0 {
        return None;
    }
    let mut idx = 0;
    let mut current_line = 1usize;
    while current_line < line {
        if let Some(pos) = buf[idx..].iter().position(|b| *b == b'\n') {
            idx += pos + 1;
            current_line += 1;
        } else {
            return None;
        }
    }
    let line_end = buf[idx..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|p| idx + p)
        .unwrap_or(buf.len());
    let line_len = line_end - idx;
    let col = character.saturating_sub(1).min(line_len);
    Some(idx + col)
}

/// Extract text at a given byte range from a file
fn extract_text_at_offset(file_path: &str, start: usize, end: usize) -> Option<String> {
    let content = std::fs::read(file_path).ok()?;
    if start >= content.len() || end > content.len() || start >= end {
        return None;
    }
    String::from_utf8(content[start..end].to_vec()).ok()
}

/// Get the full line containing a given byte offset
fn get_line_at_offset(file_path: &str, offset: usize) -> Option<String> {
    let content = std::fs::read(file_path).ok()?;
    if offset >= content.len() {
        return None;
    }

    // Find line start (search backwards for newline or start of file)
    let line_start = content[..offset]
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|p| p + 1)
        .unwrap_or(0);

    // Find line end (search forwards for newline or end of file)
    let line_end = content[offset..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|p| offset + p)
        .unwrap_or(content.len());

    String::from_utf8(content[line_start..line_end].to_vec()).ok()
}

/// Summary information for a file's structure
struct FileSummary {
    /// Symbol counts by kind (e.g., "function" -> 45)
    counts_by_kind: Vec<(String, usize)>,
    /// Total line count in the file
    line_count: usize,
    /// Key types: public types with many methods, sorted by method count
    key_types: Vec<String>,
}

/// Compute summary statistics for a file's symbols
fn compute_file_summary(symbols: &[SymbolRecord], file_path: &str) -> FileSummary {
    use std::collections::HashMap;

    // Count symbols by kind
    let mut kind_counts: HashMap<String, usize> = HashMap::new();
    for sym in symbols {
        *kind_counts.entry(sym.kind.clone()).or_default() += 1;
    }

    // Sort by count descending, then by kind name
    let mut counts_by_kind: Vec<(String, usize)> = kind_counts.into_iter().collect();
    counts_by_kind.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    // Count lines in the file
    let line_count = fs::read_to_string(file_path)
        .map(|content| content.lines().count())
        .unwrap_or(0);

    // Find key types: public structs/classes/traits/interfaces with methods
    // Count methods per container
    let mut methods_per_container: HashMap<String, usize> = HashMap::new();
    for sym in symbols {
        if let Some(ref container) = sym.container {
            if sym.kind == "function" || sym.kind == "method" {
                *methods_per_container.entry(container.clone()).or_default() += 1;
            }
        }
    }

    // Filter to public types and sort by method count
    let type_kinds = ["struct", "class", "trait", "interface", "enum", "type"];
    let mut type_symbols: Vec<(&SymbolRecord, usize)> = symbols
        .iter()
        .filter(|s| {
            type_kinds.contains(&s.kind.as_str())
                && s.visibility.as_ref().map(|v| v == "pub").unwrap_or(false)
        })
        .map(|s| {
            let method_count = methods_per_container.get(&s.name).copied().unwrap_or(0);
            (s, method_count)
        })
        .collect();

    // Sort by method count descending
    type_symbols.sort_by(|a, b| b.1.cmp(&a.1));

    // Take top 5 key types, or types with 3+ methods
    let key_types: Vec<String> = type_symbols
        .into_iter()
        .filter(|(_, count)| *count >= 3)
        .take(5)
        .map(|(sym, count)| {
            if count > 0 {
                format!("{} ({} methods)", sym.name, count)
            } else {
                sym.name.clone()
            }
        })
        .collect();

    FileSummary {
        counts_by_kind,
        line_count,
        key_types,
    }
}

/// Format the file summary as a compact string
fn format_file_summary(summary: &FileSummary) -> String {
    let mut parts = Vec::new();

    // Format counts by kind
    for (kind, count) in &summary.counts_by_kind {
        let plural = if *count == 1 { "" } else { "s" };
        parts.push(format!("{} {}{}", count, kind, plural));
    }

    let counts_str = parts.join(", ");
    let lines_str = format!("{} lines", summary.line_count);

    let mut result = format!("Summary: {} | {}", counts_str, lines_str);

    if !summary.key_types.is_empty() {
        result.push_str(&format!("\nKey types: {}", summary.key_types.join(", ")));
    }

    result
}

/// Build a hierarchical tree of symbols for the structure command
fn build_structure_tree(symbols: &[SymbolRecord], _file_path: &str) -> Result<Vec<Value>> {
    use std::collections::HashMap;

    // Convert each symbol to a minimal JSON node (name, kind, start_line only)
    let mut nodes: Vec<(Option<String>, Value)> = Vec::new();

    for sym in symbols {
        let (start_line, _) = offset_to_line_col_in_file(&sym.file, sym.start as usize)?;

        let node = json!({
            "name": sym.name,
            "kind": sym.kind,
            "line": start_line
        });

        nodes.push((sym.container.clone(), node));
    }

    // Group children by their container name
    let mut children_by_container: HashMap<String, Vec<Value>> = HashMap::new();
    let mut roots: Vec<Value> = Vec::new();

    for (container, node) in nodes {
        if let Some(container_name) = container {
            children_by_container
                .entry(container_name)
                .or_default()
                .push(node);
        } else {
            roots.push(node);
        }
    }

    // Recursively attach children to their parents
    fn attach_children(node: &mut Value, children_map: &mut HashMap<String, Vec<Value>>) {
        if let Some(name) = node.get("name").and_then(|n| n.as_str()) {
            if let Some(children) = children_map.remove(name) {
                let mut children_array: Vec<Value> = children;
                for child in &mut children_array {
                    attach_children(child, children_map);
                }
                // Sort children by line number
                children_array.sort_by(|a, b| {
                    let a_line = a.get("line").and_then(|l| l.as_u64()).unwrap_or(0);
                    let b_line = b.get("line").and_then(|l| l.as_u64()).unwrap_or(0);
                    a_line.cmp(&b_line)
                });
                node["children"] = json!(children_array);
            }
        }
    }

    for root in &mut roots {
        attach_children(root, &mut children_by_container);
    }

    // Any remaining orphans become roots
    for (_, orphans) in children_by_container {
        roots.extend(orphans);
    }

    // Sort roots by line number
    roots.sort_by(|a, b| {
        let a_line = a.get("line").and_then(|l| l.as_u64()).unwrap_or(0);
        let b_line = b.get("line").and_then(|l| l.as_u64()).unwrap_or(0);
        a_line.cmp(&b_line)
    });

    Ok(roots)
}

/// Abbreviate symbol kinds for compact output
fn abbreviate_kind(kind: &str) -> &'static str {
    match kind {
        "function" => "fn",
        "struct" => "st",
        "interface" => "if",
        "type" => "ty",
        "enum" => "en",
        "trait" => "tr",
        "method" => "me",
        "const" => "cn",
        "variable" => "va",
        "class" => "cl",
        _ => "??",
    }
}

/// Format the structure tree as ultra-compact text (~4-5 tokens/line)
/// Format: `<name> <kind_abbrev> <line>` with single-space indent for children
fn format_structure_tree(nodes: &[Value], indent: usize, output: &mut String) {
    use std::fmt::Write;

    for node in nodes {
        let name = node.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
        let line = node.get("line").and_then(|l| l.as_u64()).unwrap_or(0);

        let kind_abbrev = abbreviate_kind(kind);
        let indent_str = " ".repeat(indent);

        let _ = writeln!(output, "{}{} {} {}", indent_str, name, kind_abbrev, line);

        if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
            format_structure_tree(children, indent + 1, output);
        }
    }
}

/// Run the MCP server with the given workspace and database paths.
pub fn run_server(workspace_root: &Path, db_path: &Path) -> Result<()> {
    let mut server = McpServer::new(workspace_root.to_path_buf(), db_path.to_path_buf());
    server.run()
}
