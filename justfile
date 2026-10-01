set shell := ["bash", "-cu"]

[private]
default: help

# Show available recipes
help:
    @just --list --unsorted

# Build clax and start a harness on ~/.clax-dev and port 7481 (claude and pi load this checkout's plugin; codex uses the installed one); with no harness, `just watch`
[positional-arguments]
dev *ARGS:
    ./scripts/dev.sh "$@"

# Run the auto-reloading daemon and web UI (~/.clax-dev on 7481, or $CLAX_DEV_PORT; --shared: ~/.clax on 7480)
[positional-arguments]
watch *ARGS:
    ./scripts/watch.sh "$@"

# Build the Rust workspace
build:
    cargo build --workspace

# Install web dependencies and build the frontend bundles
web:
    cd web && npm ci
    cd web && node scripts/clean-dist.mjs
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

# Run the plugin wrapper tests (scripts/ensure-clax.sh)
wrapper-test:
    ./scripts/test-ensure-clax.sh

# Run the release installer tests (install.sh)
install-test:
    ./scripts/test-install.sh

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

# Install clax from this checkout into $CARGO_HOME/bin (~/.cargo/bin), stop the agents' daemon if it runs that binary (the next agent call starts the new build), and register the plugins with each harness found
install: web
    cargo install --locked --root "${CARGO_HOME:-$HOME/.cargo}" --path crates/clax-cli
    . scripts/dev-home.sh && stop_installed_daemon "${CLAX_HOME:-$HOME/.clax}" "${CARGO_HOME:-$HOME/.cargo}/bin/clax"
    "${CARGO_HOME:-$HOME/.cargo}/bin/clax" init

# Remove the plugin registrations, stop the agents' daemon if it runs the installed clax, and remove that clax
uninstall:
    -"${CARGO_HOME:-$HOME/.cargo}/bin/clax" uninit
    -. scripts/dev-home.sh && stop_installed_daemon "${CLAX_HOME:-$HOME/.clax}" "${CARGO_HOME:-$HOME/.cargo}/bin/clax"
    -cargo uninstall --root "${CARGO_HOME:-$HOME/.cargo}" clax-cli

# Run the dev daemon in the foreground on ~/.clax-dev, port 7481 (extra args go to `clax serve`)
serve *ARGS:
    CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}" cargo run -p clax-cli -- serve --foreground --port 7481 {{ARGS}}

# Stop the dev daemon (~/.clax-dev)
stop:
    CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}" cargo run -q -p clax-cli -- stop

# Check the dev home and its daemon (~/.clax-dev)
doctor:
    CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}" cargo run -q -p clax-cli -- doctor

# Remove build output and web dependencies
clean:
    cargo clean
    rm -rf web/dist/_clax web/dist/.vite web/dist/index.html web/dist/artifact.html web/node_modules
