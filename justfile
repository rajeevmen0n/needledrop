# Run everything from inside the dev shell: `nix develop -c just <recipe>`.
# All recipes run from the repo root, so the server finds `config.toml` and `data/` there.

manifest := "--manifest-path server/Cargo.toml"

# List the recipes
default:
    @just --list

# Server with auto-reload plus the Vite dev server; Ctrl-C stops both
dev: install
    #!/usr/bin/env bash
    set -euo pipefail
    # Job control gives each background job its own process group, so the
    # whole tree (bacon -> cargo -> server, pnpm -> vite) can be killed at once.
    set -m
    bacon --headless -j server </dev/null &
    server=$!
    pnpm --dir web dev </dev/null &
    web=$!
    trap 'kill -- -"$server" -"$web" 2>/dev/null || true; wait' EXIT
    trap 'exit 0' INT TERM
    # Stop as soon as either one exits.
    wait -n || true

# Server only, no auto-reload
server:
    cargo run {{ manifest }}

# Vite dev server only
web: install
    pnpm --dir web dev

# Install the web dependencies from the lockfile
install:
    pnpm --dir web install --frozen-lockfile

# Server unit tests
test:
    cargo test {{ manifest }}

# Everything that must pass before a commit (together with `just test`)
check: install
    cargo clippy {{ manifest }} --all-targets -- -D warnings
    cargo fmt {{ manifest }} --check
    pnpm --dir web check
    pnpm --dir web build

# Release build of the server and the production client bundle (web/dist)
build: install
    cargo build {{ manifest }} --release
    pnpm --dir web build

# Format the Rust code
fmt:
    cargo fmt {{ manifest }}
