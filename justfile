set shell := ["bash", "-cu"]

[private]
default: help

# Show available recipes
help:
    @just --list --unsorted

# Build clax and start a harness on ~/.clax-dev and port 7481 (claude and pi load this checkout's plugin; codex and grok use the installed one); with no harness, `just watch`
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
    ./scripts/build-web.sh

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

# Run the Rust workspace tests (the shell tests need `just web` once first); cargo nextest when installed
test:
    #!/usr/bin/env bash
    set -euo pipefail
    if cargo nextest --version >/dev/null 2>&1; then
        # One prebuilt clax for the tests that cannot name it (CLAX_TEST_BIN):
        # nextest runs each test in its own process, and each would build it.
        cargo build -q -p clax-cli
        mkdir -p target/clax-test
        cp target/debug/clax target/clax-test/clax
        CLAX_TEST_BIN="$PWD/target/clax-test/clax" cargo nextest run --workspace
    else
        cargo test --workspace
    fi

# Run clippy with warnings denied, with and without the test targets, plus the web lint
lint:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy --workspace -- -D warnings
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

# Run the perf gates' full versions (the quality gates run their quick versions): daemon latency, realtime clients, time to usable
perf: web
    cargo build -q --release -p clax-cli
    CLAX_PERF_BIN="$PWD/target/release/clax" ./scripts/perf-daemon.sh
    CLAX_PERF_BIN="$PWD/target/release/clax" ./scripts/perf-clients.sh
    cd web && npx playwright install chromium >/dev/null && npm run perf

# Install clax from this checkout into $CARGO_HOME/bin (~/.cargo/bin), stop the agents' daemon if it runs that binary (the next agent call starts the new build), register the plugins with each harness found, and point the plugins at that binary (clax init sets the bin setting)
install: web
    cargo install --locked --root "${CARGO_HOME:-$HOME/.cargo}" --path crates/clax-cli
    . scripts/dev-home.sh && stop_installed_daemon "${CLAX_HOME:-$HOME/.clax}" "${CARGO_HOME:-$HOME/.cargo}/bin/clax"
    "${CARGO_HOME:-$HOME/.cargo}/bin/clax" init

# Remove the plugin registrations and the bin setting naming the installed clax (clax uninit), stop the agents' daemon if it runs that clax, and remove it
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

# Try rooms and sample() in a scratch daemon (the stub provider unless ANTHROPIC_API_KEY is set)
demo-room-sample:
    scripts/demo-room-sample.sh

# Remove build output and web dependencies
clean:
    cargo clean
    rm -rf web/dist/_clax web/dist/.vite web/dist/index.html web/dist/artifact.html web/node_modules
