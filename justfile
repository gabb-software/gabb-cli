# Run all CI checks (use before committing)
ci: fmt-check lint test
    @echo "All CI checks passed!"

# Run all CI checks including release build
ci-full: fmt-check lint test build
    @echo "All CI checks passed!"

# Format code
fmt:
    cargo fmt --all

# Check formatting (without modifying)
fmt-check:
    cargo fmt --all -- --check

# Run clippy lints
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# Run tests
test:
    cargo test --all-features

# Build release binary
build:
    cargo build --release

# Build debug binary
build-debug:
    cargo build

# Clean build artifacts
clean:
    cargo clean

# Run the daemon on current directory
run *ARGS:
    cargo run -- daemon --root . --db .gabb/index.db {{ARGS}}

# Run the daemon with rebuild flag
run-rebuild:
    cargo run -- daemon --root . --db .gabb/index.db --rebuild

# List symbols in current index
symbols *ARGS:
    cargo run -- symbols --db .gabb/index.db {{ARGS}}

# Show help
help:
    @just --list
