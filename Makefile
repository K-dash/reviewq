.PHONY: all check fmt lint test build clean test-hooks

# Default target: run all
all: fmt lint test test-hooks

# Format
fmt:
	cargo fmt

# Format check (for CI)
fmt-check:
	cargo fmt --check

# Lint (clippy)
lint:
	cargo clippy -- -D warnings

# Test
test:
	cargo test

# Build
build:
	cargo build

# Release build
release:
	cargo build --release

# Check (compile only, no binary generation)
check:
	cargo check

# Clean
clean:
	cargo clean

# Workflow enforcement hook self-tests. See .claude/hooks/README.md.
test-hooks:
	@bash .claude/hooks/tests/run-tests.sh

# For CI: fmt-check + lint + test + hooks
ci: fmt-check lint test test-hooks
