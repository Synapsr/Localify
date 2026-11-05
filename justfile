# Localify Justfile - Development task automation

# Default recipe (show available commands)
default:
    @just --list

# Run all tests
test:
    cargo test

# Run tests with output
test-verbose:
    cargo test -- --nocapture

# Run formatting and linting checks (CI equivalent)
check: fmt-check clippy
    @echo "✓ All checks passed!"

# Check code formatting
fmt-check:
    cargo fmt --check

# Run clippy lints
clippy:
    cargo clippy -- -D warnings

# Auto-fix formatting and clippy warnings
fix:
    cargo fmt
    cargo clippy --fix --allow-dirty --allow-staged

# Build optimized release binary
build-release:
    cargo build --release

# Run localify daemon (requires installation)
run:
    cargo run

# Run security audit
audit:
    cargo audit

# Clean build artifacts
clean:
    cargo clean

# Install development dependencies
install-deps:
    @echo "Installing development dependencies..."
    @echo ""
    @echo "Installing cargo-audit..."
    cargo install cargo-audit
    @echo ""
    @echo "✓ All dependencies installed!"
