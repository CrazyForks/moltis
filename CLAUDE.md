# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## General

This is doing a Rust version of moltbot. Moltbot documentation is available at
https://docs.molt.bot/ and a local repository is in `../clawdbot/`

Dig this repo and documentation to figure out how moltbot is working and how
many features it has. `../clawdbot/HOWITWORKS.md` has explaination of how it
works. But feel free to do any improvement and change the way it is to make
it more Rustacean.

Always use traits if possible, to allow other implementations.

All code you write must have test with a high coverage.

## Build and Development Commands

```bash
cargo build              # Build the project
cargo build --release    # Build with optimizations
cargo run                # Run the project
cargo run --release      # Run with optimizations
```

## Testing

```bash
cargo test                           # Run all tests
cargo test <test_name>               # Run a specific test
cargo test <module>::               # Run all tests in a module
cargo test -- --nocapture            # Run tests with stdout visible
```

## Code Quality

```bash
cargo fmt                # Format code
cargo clippy             # Run linter
cargo check              # Fast compile check without producing binary
```

## Git Workflow

Follow conventional commit format: `feat|fix|refactor|docs|test|chore(scope): description`

Run all checks before committing:
1. `cargo check`
2. `cargo clippy`
3. `cargo test`
