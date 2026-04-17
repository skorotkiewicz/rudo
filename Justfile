# Justfile
# https://github.com/casey/just

default:
    @just --list

build:
    cargo build --release

run *args:
    cargo run -- {{args}}

fmt:
    cargo fmt
    cargo clippy --all-targets --all-features -- -D warnings
    # cargo shear --fix # first install shear: cargo install shear

check:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings

install-hook:
    #!/usr/bin/env bash
    cat > .git/hooks/pre-commit << 'EOF'
    #!/bin/sh
    set -e
    echo "Running pre-commit quality checks..."
    just check
    EOF
    chmod +x .git/hooks/pre-commit
    echo "Pre-commit hook installation confirmed."

remove-hook:
    rm .git/hooks/pre-commit
    echo "Pre-commit hook uninstallation confirmed."

# Run unit tests
test: fmt
    cargo test