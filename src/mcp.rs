//! MCP (Model Context Protocol) server implementation.
//!
//! This module implements an MCP server that exposes gabb's code indexing
//! capabilities as tools for AI assistants like Claude.

use crate::store::{IndexStore, SymbolRecord};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

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

/// MCP Server state
pub struct McpServer {
    workspace_root: PathBuf,
    db_path: PathBuf,
    store: Option<IndexStore>,
    initialized: bool,
}

impl McpServer {
    pub fn new(workspace_root: PathBuf, db_path: PathBuf) -> Self {
        Self {
            workspace_root,
            db_path,
            store: None,
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
        // Ensure index is available (auto-start daemon if needed)
        self.ensure_index()?;

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
                description: "List or search symbols in the codebase. Returns functions, classes, interfaces, types, etc.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Filter by symbol name (exact match)"
                        },
                        "kind": {
                            "type": "string",
                            "description": "Filter by kind (function, class, interface, type, etc.)"
                        },
                        "file": {
                            "type": "string",
                            "description": "Filter by file path"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results (default: 50)"
                        }
                    }
                }),
            },
            Tool {
                name: "gabb_symbol".to_string(),
                description: "Get detailed information about a symbol by name.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Symbol name to find"
                        },
                        "kind": {
                            "type": "string",
                            "description": "Filter by kind (function, class, interface, type, etc.)"
                        }
                    },
                    "required": ["name"]
                }),
            },
            Tool {
                name: "gabb_definition".to_string(),
                description: "Go to definition for a symbol at a source position.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Source file path"
                        },
                        "line": {
                            "type": "integer",
                            "description": "1-based line number"
                        },
                        "character": {
                            "type": "integer",
                            "description": "1-based column number"
                        }
                    },
                    "required": ["file", "line", "character"]
                }),
            },
            Tool {
                name: "gabb_usages".to_string(),
                description: "Find all usages/references of a symbol at a source position.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Source file path"
                        },
                        "line": {
                            "type": "integer",
                            "description": "1-based line number"
                        },
                        "character": {
                            "type": "integer",
                            "description": "1-based column number"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results (default: 50)"
                        }
                    },
                    "required": ["file", "line", "character"]
                }),
            },
            Tool {
                name: "gabb_implementations".to_string(),
                description: "Find implementations of an interface, trait, or abstract class.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Source file path"
                        },
                        "line": {
                            "type": "integer",
                            "description": "1-based line number"
                        },
                        "character": {
                            "type": "integer",
                            "description": "1-based column number"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results (default: 50)"
                        }
                    },
                    "required": ["file", "line", "character"]
                }),
            },
            Tool {
                name: "gabb_daemon_status".to_string(),
                description: "Check the status of the gabb indexing daemon.".to_string(),
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

    // ==================== Tool Implementations ====================

    fn ensure_index(&mut self) -> Result<()> {
        if self.store.is_some() {
            return Ok(());
        }

        // Check if daemon is running and index exists
        use crate::daemon;

        if !self.db_path.exists() {
            // Start daemon in background
            log::info!("Index not found. Starting daemon...");
            daemon::start(&self.workspace_root, &self.db_path, false, true, None)?;

            // Wait for index to be ready
            let max_wait = std::time::Duration::from_secs(60);
            let start = std::time::Instant::now();
            while !self.db_path.exists() && start.elapsed() < max_wait {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }

            if !self.db_path.exists() {
                bail!("Daemon started but index not created within 60 seconds");
            }
        }

        // Open the store
        self.store = Some(IndexStore::open(&self.db_path)?);
        Ok(())
    }

    fn get_store(&mut self) -> Result<&IndexStore> {
        self.ensure_index()?;
        self.store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Store not initialized"))
    }

    fn tool_symbols(&mut self, args: &Value) -> Result<ToolResult> {
        let store = self.get_store()?;

        let name = args.get("name").and_then(|v| v.as_str());
        let kind = args.get("kind").and_then(|v| v.as_str());
        let file = args.get("file").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        let symbols = store.list_symbols(file, kind, name, Some(limit))?;

        if symbols.is_empty() {
            return Ok(ToolResult::text("No symbols found matching the criteria."));
        }

        let output = format_symbols(&symbols, &self.workspace_root);
        Ok(ToolResult::text(output))
    }

    fn tool_symbol(&mut self, args: &Value) -> Result<ToolResult> {
        let store = self.get_store()?;

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

        let output = format_symbols(&symbols, &self.workspace_root);
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

        let file_path = self.resolve_path(file);

        // Find symbol at position
        if let Some(symbol) = self.find_symbol_at(&file_path, line, character)? {
            let output = format_symbol(&symbol, &self.workspace_root);
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

        let file_path = self.resolve_path(file);

        // Find symbol at position
        let symbol = match self.find_symbol_at(&file_path, line, character)? {
            Some(s) => s,
            None => {
                return Ok(ToolResult::text(format!(
                    "No symbol found at {}:{}:{}",
                    file, line, character
                )));
            }
        };

        // Find references using references_for_symbol
        let store = self.get_store()?;
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
            let rel_path = self.relative_path(&r.file);
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

        let file_path = self.resolve_path(file);

        // Find symbol at position
        let symbol = match self.find_symbol_at(&file_path, line, character)? {
            Some(s) => s,
            None => {
                return Ok(ToolResult::text(format!(
                    "No symbol found at {}:{}:{}",
                    file, line, character
                )));
            }
        };

        // Find implementations via edges_to (edges pointing TO the symbol from implementations)
        let store = self.get_store()?;
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
            let output = format_symbols(&fallback, &self.workspace_root);
            return Ok(ToolResult::text(format!(
                "Implementations of '{}' (by name):\n\n{}",
                symbol.name, output
            )));
        }

        impls.truncate(limit);
        let output = format_symbols(&impls, &self.workspace_root);
        Ok(ToolResult::text(format!(
            "Implementations of '{}':\n\n{}",
            symbol.name, output
        )))
    }

    fn tool_daemon_status(&mut self) -> Result<ToolResult> {
        use crate::daemon;

        let status = if let Ok(Some(pid_info)) = daemon::read_pid_file(&self.workspace_root) {
            if daemon::is_process_running(pid_info.pid) {
                format!(
                    "Daemon: running (PID {})\nVersion: {}\nWorkspace: {}\nDatabase: {}",
                    pid_info.pid,
                    pid_info.version,
                    self.workspace_root.display(),
                    self.db_path.display()
                )
            } else {
                format!(
                    "Daemon: not running (stale PID file)\nWorkspace: {}\nDatabase: {}",
                    self.workspace_root.display(),
                    self.db_path.display()
                )
            }
        } else {
            format!(
                "Daemon: not running\nWorkspace: {}\nDatabase: {}",
                self.workspace_root.display(),
                self.db_path.display()
            )
        };

        Ok(ToolResult::text(status))
    }

    // ==================== Helper Methods ====================

    fn resolve_path(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            self.workspace_root.join(p)
        }
    }

    fn relative_path(&self, path: &str) -> String {
        let p = PathBuf::from(path);
        p.strip_prefix(&self.workspace_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string())
    }

    fn find_symbol_at(
        &mut self,
        file: &Path,
        line: usize,
        character: usize,
    ) -> Result<Option<SymbolRecord>> {
        let store = self.get_store()?;
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
