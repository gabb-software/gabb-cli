# Gabb CLI

Gabb is a Rust CLI that builds a local code index so editors and AI coding assistants can answer questions like "where is this implemented?" without shipping your sources to a remote service. The MVP focuses on TypeScript/TSX and Rust projects with an indexing daemon that stays in sync with filesystem changes.

## Status
- MVP: indexes TypeScript/TSX and Rust, storing results in a local SQLite database
- Commands: `gabb daemon` (watches a workspace and keeps the index fresh), `gabb symbols` (list indexed symbols)
- Outputs: symbol definitions, relationships (implements/extends), and references

## Quickstart
```bash
# 1) Build (or install) the CLI
cargo build        # or: cargo install --path .

# 2) Start the daemon from your project root
cargo run -- daemon --root . --db .gabb/index.db

# 3) Let the daemon watch for changes; the index lives at .gabb/index.db
```

The daemon will crawl your workspace, index all `*.ts`/`*.tsx`/`*.rs` files, and keep the SQLite database up to date as files change. Use `-v`/`-vv` to increase logging.

## Installation
- Prerequisite: Rust toolchain (Edition 2024). Install via [rustup](https://rustup.rs/).
- Install locally from source:
  ```bash
  cargo install --path .
  ```
- Or build without installing:
  ```bash
  cargo build
  ```

## Usage
```bash
gabb daemon --root <workspace> --db <path/to/index.db> [-v|-vv]
gabb symbols --db <path/to/index.db> [--file <path>] [--kind <kind>] [--limit <n>]
gabb implementation --db <path/to/index.db> --file <path> --line <line> --character <char> [--limit <n>] [--kind <kind>]
```

Flags:
- `--root`: workspace to index (defaults to current directory)
- `--db`: SQLite database path (defaults to `.gabb/index.db`)
- `-v`, `-vv`: increase log verbosity
Symbols command filters:
- `--file`: only show symbols from a given file path
- `--kind`: filter by kind (`function`, `class`, `interface`, `method`, `struct`, `enum`, `trait`)
- `--limit`: cap the number of rows returned
Implementation command:
- Requires `--file`, `--line`, and `--character` to identify the symbol under the cursor
- Tries to find implementations via recorded edges; falls back to same-name matches (best effort for now)

What gets indexed:
- Files: `*.ts`, `*.tsx`, `*.rs`
- Data stored: symbols (functions, classes, interfaces, methods), relationships (implements/extends), references
- Storage: SQLite with WAL enabled for safe concurrent reads

## Project Layout
- `src/main.rs`: CLI entrypoint and logging setup
- `src/daemon.rs`: filesystem watcher and incremental indexing loop
- `src/indexer.rs`: full/index-one routines and workspace traversal
- `src/ts.rs`: TypeScript parser built on tree-sitter
- `src/rust_lang.rs`: Rust parser built on tree-sitter
- `src/store.rs`: SQLite-backed index store
- `ARCHITECTURE.md`: deeper design notes for future commands (find implementations, usages, duplicates)

## Development
- Format and lint: `cargo fmt && cargo clippy --all-targets --all-features`
- Tests: `cargo test`
- Docs: `cargo doc --open`

## Roadmap
- Additional commands (find implementations/usages) backed by the stored relationships
- More languages by swapping in new tree-sitter grammars
- Richer queries (duplicates, unused code) atop the same index

## Contributing
Issues and PRs are welcome. Please:
- Keep commits focused and prefer Conventional Commits (`feat: ...`, `fix: ...`)
- Add or update tests when changing indexing behavior
- Run `cargo fmt`, `cargo clippy`, and `cargo test` before submitting
