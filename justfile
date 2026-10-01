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

# Install clax from this checkout into ~/.cargo/bin and register its plugins with each harness found
install: web
    cargo install --locked --path crates/clax-cli
    "${CARGO_HOME:-$HOME/.cargo}/bin/clax" init
    @b="${CARGO_HOME:-$HOME/.cargo}/bin/clax"; f="$(command -v clax || true)"; if [ "$f" != "$b" ]; then echo "warning: the first clax on PATH is ${f:-none}, not $b; the plugins run the first one, so put ${b%/clax} first on PATH" >&2; fi

# Remove the plugin registrations and the clax installed by `just install`
uninstall:
    -"${CARGO_HOME:-$HOME/.cargo}/bin/clax" uninit
    -cargo uninstall clax-cli

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
    rm -rf web/dist/_clax web/dist/index.html web/node_modules
