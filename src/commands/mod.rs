//! Command implementations for the gabb CLI.
//!
//! Each submodule handles a specific category of commands.

pub mod deps;
pub mod duplicates;
pub mod init;
pub mod mcp_config;
pub mod query;
pub mod stats;
pub mod structure;

// Re-export main command functions for convenient access
pub use deps::{find_includers, find_includes};
pub use duplicates::find_duplicates;
pub use init::{init_project, setup_wizard};
pub use mcp_config::{mcp_command, mcp_config, mcp_install, mcp_status, mcp_uninstall};
pub use query::{find_definition, find_implementation, find_usages, list_symbols, show_symbol};
pub use stats::show_stats;
pub use structure::file_structure;
