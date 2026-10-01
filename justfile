set shell := ["bash", "-cu"]

[private]
default: help

# Show available recipes
help:
    @just --list --unsorted

# Run a dev server that reloads on Rust and web changes, on its own home (~/.clax-dev) and port 7481; --shared serves the real home on 7480
dev *ARGS:
    ./scripts/dev.sh {{ARGS}}

# Build the Rust workspace
build:
    cargo build --workspace

# Install web dependencies and build the frontend bundles
web:
    cd web && npm ci
    rm -rf web/dist/_clax web/dist/index.html
    cd web && npm run build

# Run the web lint, typecheck, and unit tests
web-test:
    cd web && npm ci && npm run lint && npm run typecheck && npm test

# Build the frontend and run the Playwright end-to-end tests
web-e2e: web
    cd web && npx playwright install --with-deps chromium && npm run e2e

# Run the Pi extension tests
pi-test:
    cd plugins/pi && npm ci && npm run typecheck && npm test

# Run the installer script tests
installer-test:
    ./scripts/test-ensure-clax.sh

# Check the plugin manifests, commands, and skill
plugin-test:
    ./scripts/test-plugins.sh

# Run the Rust workspace tests (the shell tests need `just web` once first)
test:
    cargo test --workspace

# Run clippy and cargo check with warnings denied, plus the web lint
lint:
    cargo clippy --workspace --all-targets -- -D warnings
    RUSTFLAGS=-Dwarnings cargo check --workspace
    cd web && npm run lint

# Format the Rust code (the web lint is not a formatter)
fmt:
    cargo fmt --all

# Auto-format then run every quality gate (dev loop)
check *GATES:
    ./scripts/check.sh {{GATES}}

# Run the same quality gates CI runs, without auto-format
ci *GATES:
    ./scripts/quality_gates.sh {{GATES}}

# Build the frontend and install the clax binary
install: web
    cargo install --path crates/clax-cli

# Remove the installed clax binary
uninstall:
    -cargo uninstall clax-cli
    -rm ~/.local/bin/clax

# Run the daemon in the foreground (extra args go to `clax serve`)
serve *ARGS:
    cargo run -p clax-cli -- serve --foreground {{ARGS}}

# Stop the running daemon
stop:
    cargo run -q -p clax-cli -- stop

# Check the local setup and daemon health
doctor:
    cargo run -q -p clax-cli -- doctor

# Remove build output and web dependencies
clean:
    cargo clean
    rm -rf web/dist/_clax web/dist/index.html web/node_modules
