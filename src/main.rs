//! Gabb CLI - Code indexing daemon and query tool.
//!
//! This is the entry point for the gabb command-line interface.

use anyhow::Result;
use clap::Parser;
use std::path::Path;

mod cli;
mod commands;
mod output;
mod util;

use cli::{Cli, Commands, DaemonCommands, McpCommands};
use gabb_cli::daemon;
use gabb_cli::mcp;
use gabb_cli::workspace;
use gabb_cli::ExitCode;
use output::SourceDisplayOptions;

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose, cli.quiet);
    let format = cli.format;
    let quiet = cli.quiet;

    // Resolve workspace and database paths
    let workspace = match workspace::resolve_workspace(cli.workspace.as_deref()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::Error.into();
        }
    };
    let db = workspace::resolve_db_path(cli.db.as_deref(), &workspace);

    // Set environment variables for child processes
    workspace::set_env_for_children(&workspace, &db);

    let result = match cli.command {
        Commands::Daemon { command } => match command {
            DaemonCommands::Start {
                rebuild,
                background,
                log_file,
                quiet: daemon_quiet,
            } => daemon::start(
                &workspace,
                &db,
                rebuild,
                background,
                log_file.as_deref(),
                daemon_quiet || quiet,
            )
            .map(|_| ExitCode::Success),
            DaemonCommands::Stop { force } => {
                daemon::stop(&workspace, force).map(|_| ExitCode::Success)
            }
            DaemonCommands::Restart { rebuild } => {
                daemon::restart(&workspace, &db, rebuild).map(|_| ExitCode::Success)
            }
            DaemonCommands::Status => daemon::status(&workspace, format).map(|_| ExitCode::Success),
        },
        Commands::Symbols {
            file,
            kind,
            name,
            fuzzy,
            limit,
            offset,
            source,
            context,
        } => ensure_index(&db, cli.no_start_daemon, cli.no_daemon).and_then(|_| {
            let source_opts = SourceDisplayOptions {
                include_source: source,
                context_lines: context,
            };
            commands::list_symbols(
                &db,
                file.as_ref(),
                kind.as_deref(),
                name.as_deref(),
                fuzzy,
                limit,
                offset,
                format,
                source_opts,
                quiet,
            )
        }),
        Commands::Implementation {
            file,
            line,
            character,
            limit,
            kind,
            source,
            context,
        } => ensure_index(&db, cli.no_start_daemon, cli.no_daemon).and_then(|_| {
            let source_opts = SourceDisplayOptions {
                include_source: source,
                context_lines: context,
            };
            commands::find_implementation(
                &db,
                &file,
                line,
                character,
                limit,
                kind.as_deref(),
                format,
                source_opts,
                quiet,
            )
        }),
        Commands::Usages {
            file,
            line,
            character,
            limit,
            source,
            context,
        } => ensure_index(&db, cli.no_start_daemon, cli.no_daemon).and_then(|_| {
            let source_opts = SourceDisplayOptions {
                include_source: source,
                context_lines: context,
            };
            commands::find_usages(
                &db,
                &file,
                line,
                character,
                limit,
                format,
                source_opts,
                quiet,
            )
        }),
        Commands::Symbol {
            name,
            file,
            kind,
            fuzzy,
            limit,
            source,
            context,
        } => ensure_index(&db, cli.no_start_daemon, cli.no_daemon).and_then(|_| {
            let source_opts = SourceDisplayOptions {
                include_source: source,
                context_lines: context,
            };
            commands::show_symbol(
                &db,
                &name,
                file.as_ref(),
                kind.as_deref(),
                fuzzy,
                limit,
                format,
                source_opts,
                quiet,
            )
        }),
        Commands::Definition {
            file,
            line,
            character,
            source,
            context,
        } => ensure_index(&db, cli.no_start_daemon, cli.no_daemon).and_then(|_| {
            let source_opts = SourceDisplayOptions {
                include_source: source,
                context_lines: context,
            };
            commands::find_definition(&db, &file, line, character, format, source_opts, quiet)
        }),
        Commands::Duplicates {
            uncommitted,
            staged,
            kind,
            min_count,
        } => ensure_index(&db, cli.no_start_daemon, cli.no_daemon).and_then(|_| {
            commands::find_duplicates(&db, uncommitted, staged, kind.as_deref(), min_count, format, quiet)
        }),
        Commands::McpServer => mcp::run_server(&workspace, &db).map(|_| ExitCode::Success),
        Commands::Mcp { command } => match command {
            McpCommands::Config { output } => {
                commands::mcp_config(&workspace, output).map(|_| ExitCode::Success)
            }
            McpCommands::Install {
                claude_desktop,
                claude_code,
            } => commands::mcp_install(&workspace, claude_desktop, claude_code)
                .map(|_| ExitCode::Success),
            McpCommands::Status { dry_run } => {
                commands::mcp_status(&workspace, &db, dry_run).map(|_| ExitCode::Success)
            }
            McpCommands::Uninstall {
                claude_desktop,
                claude_code,
            } => commands::mcp_uninstall(claude_desktop, claude_code).map(|_| ExitCode::Success),
            McpCommands::Command => commands::mcp_command(&workspace).map(|_| ExitCode::Success),
        },
        Commands::Init {
            mcp,
            gitignore,
            skill,
            claudemd,
        } => commands::init_project(&workspace, mcp, gitignore, skill, claudemd)
            .map(|_| ExitCode::Success),
        Commands::Setup {
            yes,
            dry_run,
            no_index,
        } => commands::setup_wizard(&workspace, &db, yes, dry_run, no_index)
            .map(|_| ExitCode::Success),
        Commands::Includers {
            file,
            transitive,
            limit,
        } => ensure_index(&db, cli.no_start_daemon, cli.no_daemon)
            .and_then(|_| commands::find_includers(&db, &file, transitive, limit, format, quiet)),
        Commands::Includes {
            file,
            transitive,
            limit,
        } => ensure_index(&db, cli.no_start_daemon, cli.no_daemon)
            .and_then(|_| commands::find_includes(&db, &file, transitive, limit, format, quiet)),
        Commands::Structure { file } => ensure_index(&db, cli.no_start_daemon, cli.no_daemon)
            .and_then(|_| commands::file_structure(&db, &workspace, &file, format, quiet)),
        Commands::Stats => ensure_index(&db, cli.no_start_daemon, cli.no_daemon)
            .and_then(|_| commands::show_stats(&db, format).map(|_| ExitCode::Success)),
    };

    match result {
        Ok(code) => code.into(),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::Error.into()
        }
    }
}

fn init_logging(verbosity: u8, quiet: bool) {
    let level = if quiet {
        "warn" // In quiet mode, only show warnings and errors
    } else {
        match verbosity {
            0 => "info",
            1 => "debug",
            _ => "trace",
        }
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level))
        .format_timestamp_secs()
        .init();
}

/// Ensure the index is available (wrapper for util function)
fn ensure_index(db: &Path, no_start_daemon: bool, no_daemon: bool) -> Result<()> {
    util::ensure_index_available(db, no_start_daemon, no_daemon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gabb_cli::indexer;
    use gabb_cli::offset_to_line_col;
    use gabb_cli::store::IndexStore;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn resolves_symbol_at_position() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file_path = root.join("sample.ts");
        fs::write(&file_path, "function foo() {}\nfunction bar() {}\n").unwrap();
        let file_path = file_path.canonicalize().unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

        let symbol = util::resolve_symbol_at(&store, &file_path, 1, 10).unwrap();
        assert_eq!(symbol.name, "foo");
        assert_eq!(symbol.kind, "function");
    }

    #[test]
    fn resolves_reference_by_name_when_not_a_definition() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let impl_path = root.join("indexer.rs");
        let caller_path = root.join("daemon.rs");
        fs::write(
            &impl_path,
            r#"
                fn build_full_index() {}
            "#,
        )
        .unwrap();
        let call_src = r#"fn main() { build_full_index(); }"#;
        fs::write(&caller_path, call_src).unwrap();
        let caller_path = caller_path.canonicalize().unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

        let offset = call_src.find("build_full_index").unwrap();
        let (line, character) = offset_to_line_col(call_src.as_bytes(), offset).unwrap();

        let symbol = util::resolve_symbol_at(&store, &caller_path, line, character).unwrap();
        assert_eq!(symbol.name, "build_full_index");
        assert!(symbol.file.ends_with("indexer.rs"));
    }

    #[test]
    fn usages_skip_definition_span() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file_path = root.join("foo.ts");
        let source = "function foo() {}\nfoo();\n";
        fs::write(&file_path, source).unwrap();
        let file_path = file_path.canonicalize().unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

        let symbol = util::resolve_symbol_at(&store, &file_path, 1, 10).unwrap();
        let refs = util::search_usages_by_name(&store, &symbol, root).unwrap();
        assert!(
            refs.iter().all(|r| !(r.file == symbol.file
                && r.start >= symbol.start
                && r.end <= symbol.end)),
            "should not return reference within definition"
        );
    }

    #[test]
    fn finds_usages_across_files_via_fallback() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let def_path = root.join("lib.rs");
        let caller_path = root.join("main.rs");
        fs::write(
            &def_path,
            r#"
                pub fn build_full_index() {}
            "#,
        )
        .unwrap();
        fs::write(
            &caller_path,
            r#"
                fn main() {
                    build_full_index();
                }
            "#,
        )
        .unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

        let def_path = def_path.canonicalize().unwrap();
        let src = fs::read_to_string(&def_path).unwrap();
        let offset = src.find("build_full_index").unwrap();
        let (line, character) = offset_to_line_col(src.as_bytes(), offset).unwrap();
        let symbol = util::resolve_symbol_at(&store, &def_path, line, character).unwrap();
        let refs = util::search_usages_by_name(&store, &symbol, root).unwrap();
        assert!(
            refs.iter().any(|r| r.file.ends_with("main.rs")),
            "expected a usage in main.rs, got {:?}",
            refs
        );
    }

    #[test]
    fn parses_line_character_from_file_arg() {
        use std::path::PathBuf;
        let file = PathBuf::from("src/daemon.rs:18:5");
        let (path, line, character) =
            util::parse_file_position(file.as_path(), None, None).unwrap();
        assert_eq!(path, PathBuf::from("src/daemon.rs"));
        assert_eq!(line, 18);
        assert_eq!(character, 5);

        // Explicit args override embedded position.
        let (path2, line2, character2) =
            util::parse_file_position(file.as_path(), Some(1), Some(2)).unwrap();
        assert_eq!(path2, PathBuf::from("src/daemon.rs"));
        assert_eq!(line2, 1);
        assert_eq!(character2, 2);
    }

    #[test]
    fn detects_test_files_correctly() {
        use gabb_cli::is_test_file;

        // Directory-based patterns
        assert!(is_test_file("/project/tests/foo.rs"));
        assert!(is_test_file("/project/src/__tests__/component.test.ts"));
        assert!(is_test_file("/project/test/helper.ts"));
        assert!(is_test_file("/project/spec/models.spec.ts"));

        // Rust file patterns
        assert!(is_test_file("/project/src/indexer_test.rs"));
        assert!(is_test_file("/project/src/store_spec.rs"));

        // TypeScript/JavaScript file patterns
        assert!(is_test_file("/project/src/utils.test.ts"));
        assert!(is_test_file("/project/src/utils.spec.ts"));
        assert!(is_test_file("/project/src/component.test.tsx"));
        assert!(is_test_file("/project/src/component.spec.tsx"));
        assert!(is_test_file("/project/src/helper.test.js"));
        assert!(is_test_file("/project/src/helper.spec.jsx"));

        // Production files (should return false)
        assert!(!is_test_file("/project/src/main.rs"));
        assert!(!is_test_file("/project/src/lib.rs"));
        assert!(!is_test_file("/project/src/utils.ts"));
        assert!(!is_test_file("/project/src/component.tsx"));
        assert!(!is_test_file("/project/src/index.js"));

        // Edge cases
        assert!(!is_test_file("/project/src/testing.ts")); // "testing" != "test"
        assert!(!is_test_file("/project/src/contest.ts")); // contains "test" but not a test file
    }
}
