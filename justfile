# Default recipe (runs when just is called without arguments)
default:
    @just --list

# Format Rust code
format:
    cargo +nightly fmt --all

# Check if code is formatted
format-check:
    cargo +nightly fmt -- --check

# Lint Rust code using clippy
lint:
    cargo clippy --bins --tests --benches --examples --all-features --all-targets -- -D warnings

# Build the project
build:
    cargo build

# Build in release mode
build-release:
    cargo build --release
