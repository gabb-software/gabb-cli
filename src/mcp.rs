//! MCP (Model Context Protocol) server implementation.
//!
//! This module implements an MCP server that exposes gabb's code indexing
//! capabilities as tools for AI assistants like Claude.
//!
//! Supports dynamic workspace detection - workspaces are automatically inferred
//! from file paths passed to tools, enabling one MCP server to handle multiple
//! projects.

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
                    "Supports TypeScript, Rust, Kotlin, and C++."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Filter by symbol name (exact match). Use this when you know the exact name."
                        },
                        "name_pattern": {
                            "type": "string",
                            "description": "Filter by glob-style pattern (e.g., 'get*', '*Handler', '*User*'). Use * as wildcard."
                        },
                        "name_contains": {
                            "type": "string",
                            "description": "Filter by substring (e.g., 'User' matches 'getUser', 'UserService', 'createUser')."
                        },
                        "name_fts": {
                            "type": "string",
                            "description": "Fuzzy/prefix search using FTS5 trigram matching. Supports prefix patterns (e.g., 'getUser*') and fuzzy substrings (e.g., 'usrsvc' matches 'UserService'). More flexible than exact name matching."
                        },
                        "case_insensitive": {
                            "type": "boolean",
                            "description": "Make name matching case-insensitive. Applies to name, name_pattern, and name_contains."
                        },
                        "kind": {
                            "type": "string",
                            "description": "Filter by symbol kind: function, class, interface, type, struct, enum, trait, method, const, variable"
                        },
                        "file": {
                            "type": "string",
                            "description": "Filter by file path. Supports: exact path ('src/main.ts'), directory ('src/' or 'src/components'), or glob pattern ('src/**/*.ts', '*.test.ts')."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results (default: 50). Increase for comprehensive searches."
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Number of results to skip (for offset-based pagination). Use with limit for paging through results."
                        },
                        "after": {
                            "type": "string",
                            "description": "Cursor for keyset pagination (symbol ID to start after). More efficient than offset for large result sets. Get from last result's ID."
                        },
                        "include_source": {
                            "type": "boolean",
                            "description": "Include the symbol's source code in the output. Useful for seeing implementation details."
                        },
                        "context_lines": {
                            "type": "integer",
                            "description": "Number of lines to show before and after the symbol (like grep -C). Only applies when include_source is true."
                        },
                        "namespace": {
                            "type": "string",
                            "description": "Filter by namespace/qualifier prefix (e.g., 'std::collections', 'myapp::services'). Supports glob patterns (e.g., 'std::*' for all symbols in std namespace)."
                        },
                        "scope": {
                            "type": "string",
                            "description": "Filter by containing scope/container (e.g., 'MyClass' to find methods within MyClass)."
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
                        },
                        "include_source": {
                            "type": "boolean",
                            "description": "Include the definition's source code in the output (default: true)"
                        },
                        "context_lines": {
                            "type": "integer",
                            "description": "Number of lines to show before and after the symbol (like grep -C). Only applies when include_source is true."
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
                    "and won't match comments or strings. Point to a symbol definition to find all its usages. ",
                    "Use format='refactor' for rename operations to get edit-ready output."
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
                        },
                        "format": {
                            "type": "string",
                            "enum": ["default", "refactor"],
                            "description": "Output format. Use 'refactor' for rename operations - returns JSON with exact text spans and old_text for each usage, ready for Edit tool."
                        },
                        "include_definition": {
                            "type": "boolean",
                            "description": "Include the symbol's definition location in refactor output (default: true)"
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
            Tool {
                name: "gabb_duplicates".to_string(),
                description: concat!(
                    "Find duplicate or near-duplicate code blocks in the codebase. ",
                    "USE THIS to identify copy-paste code, find refactoring opportunities, ",
                    "or detect code that should be consolidated into shared utilities. ",
                    "Groups symbols (functions, methods, classes) by identical content hash. ",
                    "Can filter by symbol kind or focus on recently changed files."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "description": "Filter by symbol kind: function, method, class, etc. Useful to focus on function duplicates."
                        },
                        "min_count": {
                            "type": "integer",
                            "description": "Minimum number of duplicates to report (default: 2). Increase to find more widespread duplication."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of duplicate groups to return (default: 20)"
                        }
                    }
                }),
            },
            Tool {
                name: "gabb_includers".to_string(),
                description: concat!(
                    "Find all files that #include this header. ",
                    "USE THIS before modifying a header to understand impact, ",
                    "or to find all compilation units affected by a change. ",
                    "Works with C/C++ headers and other languages with import/include statements."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Header file path"
                        },
                        "transitive": {
                            "type": "boolean",
                            "description": "Include transitive includers (files that include files that include this)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results to return (default: 50)"
                        }
                    },
                    "required": ["file"]
                }),
            },
            Tool {
                name: "gabb_includes".to_string(),
                description: concat!(
                    "Find all headers included by this file. ",
                    "USE THIS to understand file dependencies, ",
                    "analyze compilation complexity, or trace include chains."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Source file path"
                        },
                        "transitive": {
                            "type": "boolean",
                            "description": "Include transitive includes (follow the include chain)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results to return (default: 50)"
                        }
                    },
                    "required": ["file"]
                }),
            },
            Tool {
                name: "gabb_structure".to_string(),
                description: concat!(
                    "Get the structure of a file showing all symbols with hierarchy and positions. ",
                    "USE THIS to understand a file's organization before reading it in full. ",
                    "Returns symbols grouped hierarchically (e.g., methods inside classes) with ",
                    "start/end positions. Also indicates if file is test or production code."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Path to the file to analyze"
                        },
                        "include_source": {
                            "type": "boolean",
                            "description": "Include source code snippets in the output"
                        },
                        "context_lines": {
                            "type": "integer",
                            "description": "Lines of context around each symbol (requires include_source)"
                        }
                    },
                    "required": ["file"]
                }),
            },
            Tool {
                name: "gabb_supertypes".to_string(),
                description: concat!(
                    "Find parent types (superclasses, implemented interfaces/traits) of a type. ",
                    "USE THIS when you need to understand what a class inherits from or what interfaces it implements. ",
                    "Essential for understanding class hierarchies and polymorphism. ",
                    "Point to a class/struct definition to see its inheritance chain."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Path to the file containing the type definition"
                        },
                        "line": {
                            "type": "integer",
                            "description": "1-based line number of the type"
                        },
                        "character": {
                            "type": "integer",
                            "description": "1-based column number within the line"
                        },
                        "transitive": {
                            "type": "boolean",
                            "description": "Include full hierarchy chain, not just direct parents (default: false)"
                        },
                        "include_source": {
                            "type": "boolean",
                            "description": "Include source code of parent types in the output"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results to return (default: 50)"
                        }
                    },
                    "required": ["file", "line", "character"]
                }),
            },
            Tool {
                name: "gabb_subtypes".to_string(),
                description: concat!(
                    "Find child types (subclasses, implementors) of a type/interface/trait. ",
                    "USE THIS when you need to understand what inherits from or implements a type. ",
                    "Essential for impact analysis when modifying base classes or interfaces. ",
                    "Point to an interface/trait/class definition to find all derived types."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Path to the file containing the type/interface/trait definition"
                        },
                        "line": {
                            "type": "integer",
                            "description": "1-based line number of the type"
                        },
                        "character": {
                            "type": "integer",
                            "description": "1-based column number within the line"
                        },
                        "transitive": {
                            "type": "boolean",
                            "description": "Include full hierarchy chain, not just direct children (default: false)"
                        },
                        "include_source": {
                            "type": "boolean",
                            "description": "Include source code of child types in the output"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results to return (default: 50)"
                        }
                    },
                    "required": ["file", "line", "character"]
                }),
            },
            Tool {
                name: "gabb_rename".to_string(),
                description: concat!(
                    "Get all locations that need to be updated when renaming a symbol. ",
                    "USE THIS for safe, automated rename refactoring. Returns edit-ready JSON output with ",
                    "exact text spans, old_text, and new_text for each location. ",
                    "Includes both the definition and all usages. Output is structured for direct use with Edit tool."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Path to the file containing the symbol to rename"
                        },
                        "line": {
                            "type": "integer",
                            "description": "1-based line number of the symbol"
                        },
                        "character": {
                            "type": "integer",
                            "description": "1-based column number within the line"
                        },
                        "new_name": {
                            "type": "string",
                            "description": "The new name for the symbol"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum locations to return (default: 100)"
                        }
                    },
                    "required": ["file", "line", "character", "new_name"]
                }),
            },
            Tool {
                name: "gabb_callers".to_string(),
                description: concat!(
                    "Find all functions/methods that call a given function/method. ",
                    "USE THIS when you want to understand who calls a function, trace execution flow backwards, ",
                    "or assess impact before modifying a function. Point to a function definition to see all its callers. ",
                    "Use transitive=true to get the full call chain (callers of callers)."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Path to the file containing the function definition"
                        },
                        "line": {
                            "type": "integer",
                            "description": "1-based line number of the function"
                        },
                        "character": {
                            "type": "integer",
                            "description": "1-based column number within the line"
                        },
                        "transitive": {
                            "type": "boolean",
                            "description": "Include full call chain, not just direct callers (default: false)"
                        },
                        "include_source": {
                            "type": "boolean",
                            "description": "Include source code of caller functions in the output"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results to return (default: 50)"
                        }
                    },
                    "required": ["file", "line", "character"]
                }),
            },
            Tool {
                name: "gabb_callees".to_string(),
                description: concat!(
                    "Find all functions/methods called by a given function/method. ",
                    "USE THIS when you want to understand what a function does, trace execution flow forwards, ",
                    "or explore function dependencies. Point to a function definition to see all functions it calls. ",
                    "Use transitive=true to get the full call chain (callees of callees)."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Path to the file containing the function definition"
                        },
                        "line": {
                            "type": "integer",
                            "description": "1-based line number of the function"
                        },
                        "character": {
                            "type": "integer",
                            "description": "1-based column number within the line"
                        },
                        "transitive": {
                            "type": "boolean",
                            "description": "Include full call chain, not just direct callees (default: false)"
                        },
                        "include_source": {
                            "type": "boolean",
                            "description": "Include source code of callee functions in the output"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results to return (default: 50)"
                        }
                    },
                    "required": ["file", "line", "character"]
                }),
            },
            Tool {
                name: "gabb_stats".to_string(),
                description: concat!(
                    "Get comprehensive index statistics including file counts by language, ",
                    "symbol counts by kind, index size, last update time, and schema version. ",
                    "USE THIS to understand the scope of the indexed codebase, verify indexing is complete, ",
                    "or diagnose issues with the index."
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
            "gabb_duplicates" => self.tool_duplicates(&arguments),
            "gabb_includers" => self.tool_includers(&arguments),
            "gabb_includes" => self.tool_includes(&arguments),
            "gabb_structure" => self.tool_structure(&arguments),
            "gabb_supertypes" => self.tool_supertypes(&arguments),
            "gabb_subtypes" => self.tool_subtypes(&arguments),
            "gabb_rename" => self.tool_rename(&arguments),
            "gabb_callers" => self.tool_callers(&arguments),
            "gabb_callees" => self.tool_callees(&arguments),
            "gabb_stats" => self.tool_stats(),
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
            daemon::start(&info.root, &info.db_path, false, true, None, true)?; // quiet=true for background

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
            if let Ok((ref_line, ref_col)) = offset_to_line_col(&r.file, r.start as usize) {
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
            if let Ok((def_line, def_col)) = offset_to_line_col(&symbol.file, symbol.start as usize)
            {
                if let Ok((def_end_line, def_end_col)) =
                    offset_to_line_col(&symbol.file, symbol.end as usize)
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
            let (ref_line, ref_col) = match offset_to_line_col(&r.file, r.start as usize) {
                Ok(pos) => pos,
                Err(_) => continue,
            };

            // Get end position
            let (end_line, end_col) = match offset_to_line_col(&r.file, r.end as usize) {
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
            offset_to_line_col(&symbol.file, symbol.start as usize).unwrap_or((0, 0));
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
        let include_source = args
            .get("include_source")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let context_lines = args
            .get("context_lines")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

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

        // Determine if this is a test file
        let context = if is_test_file(&file_str) {
            "test"
        } else {
            "prod"
        };

        // Build hierarchical structure
        let tree = build_structure_tree(&symbols, include_source, context_lines, &file_str)?;

        // Build JSON output
        let output = json!({
            "file": file_str,
            "context": context,
            "symbols": tree
        });

        Ok(ToolResult::text(
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string()),
        ))
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
        if let Ok((def_line, def_col)) = offset_to_line_col(&symbol.file, symbol.start as usize) {
            if let Ok((def_end_line, def_end_col)) =
                offset_to_line_col(&symbol.file, symbol.end as usize)
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

            let (ref_line, ref_col) = match offset_to_line_col(&r.file, r.start as usize) {
                Ok(pos) => pos,
                Err(_) => continue,
            };

            let (end_line, end_col) = match offset_to_line_col(&r.file, r.end as usize) {
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
            offset_to_line_col(&symbol.file, symbol.start as usize).unwrap_or((0, 0));
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
    let location = if let Ok((line, col)) = offset_to_line_col(&sym.file, sym.start as usize) {
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

            let location =
                if let Ok((line, col)) = offset_to_line_col(&sym.file, sym.start as usize) {
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

/// Check if a file path indicates a test file
fn is_test_file(path: &str) -> bool {
    let path_lower = path.to_lowercase();

    // Check directory patterns
    if path_lower.contains("/tests/")
        || path_lower.contains("/__tests__/")
        || path_lower.contains("/test/")
        || path_lower.contains("/spec/")
    {
        return true;
    }

    // Check file name patterns
    if let Some(file_name) = path.rsplit('/').next() {
        let name_lower = file_name.to_lowercase();
        // Rust patterns
        if name_lower.ends_with("_test.rs") || name_lower.ends_with("_spec.rs") {
            return true;
        }
        // TypeScript/JavaScript patterns
        if name_lower.ends_with(".test.ts")
            || name_lower.ends_with(".spec.ts")
            || name_lower.ends_with(".test.tsx")
            || name_lower.ends_with(".spec.tsx")
            || name_lower.ends_with(".test.js")
            || name_lower.ends_with(".spec.js")
            || name_lower.ends_with(".test.jsx")
            || name_lower.ends_with(".spec.jsx")
        {
            return true;
        }
    }

    false
}

/// Build a hierarchical tree of symbols for the structure command
fn build_structure_tree(
    symbols: &[SymbolRecord],
    include_source: bool,
    context_lines: Option<usize>,
    file_path: &str,
) -> Result<Vec<Value>> {
    use std::collections::HashMap;

    // Convert each symbol to a JSON node with resolved positions
    let mut nodes: Vec<(Option<String>, Value)> = Vec::new();

    for sym in symbols {
        let (start_line, start_col) = offset_to_line_col(&sym.file, sym.start as usize)?;
        let (end_line, end_col) = offset_to_line_col(&sym.file, sym.end as usize)?;

        // Determine context from file path OR inline test markers (#[cfg(test)], #[test])
        let context = if sym.is_test || is_test_file(file_path) {
            "test"
        } else {
            "prod"
        };

        let mut node = json!({
            "name": sym.name,
            "kind": sym.kind,
            "context": context,
            "start": { "line": start_line, "character": start_col },
            "end": { "line": end_line, "character": end_col }
        });

        if let Some(vis) = &sym.visibility {
            node["visibility"] = json!(vis);
        }

        if include_source {
            if let Some(src) = extract_source(&sym.file, sym.start, sym.end, context_lines) {
                node["source"] = json!(src);
            }
        }

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
                // Sort children by start position
                children_array.sort_by(|a, b| {
                    let a_line = a
                        .get("start")
                        .and_then(|s| s.get("line"))
                        .and_then(|l| l.as_u64())
                        .unwrap_or(0);
                    let b_line = b
                        .get("start")
                        .and_then(|s| s.get("line"))
                        .and_then(|l| l.as_u64())
                        .unwrap_or(0);
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

    // Sort roots by start position
    roots.sort_by(|a, b| {
        let a_line = a
            .get("start")
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0);
        let b_line = b
            .get("start")
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0);
        a_line.cmp(&b_line)
    });

    Ok(roots)
}

/// Run the MCP server with the given workspace and database paths.
pub fn run_server(workspace_root: &Path, db_path: &Path) -> Result<()> {
    let mut server = McpServer::new(workspace_root.to_path_buf(), db_path.to_path_buf());
    server.run()
}
