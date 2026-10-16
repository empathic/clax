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

# Run the plugin wrapper tests (scripts/ensure-clax.sh)
wrapper-test:
    ./scripts/test-ensure-clax.sh

# Run the release installer tests (install.sh)
install-test:
    ./scripts/test-install.sh

# Check the plugin manifests, commands, and skill
plugin-test:
    ./scripts/test-plugins.sh

# Format the Rust code (the web lint is not a formatter)
fmt:
    cargo fmt --all

# Install clax from this checkout into $CARGO_HOME/bin (~/.cargo/bin), stop the agents' daemon if it runs that binary (the next agent call starts the new build), register the plugins with each harness found, and point the plugins at that binary (clax init sets the bin setting)
install: web-native
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

# Checks and builds run with Bazel when Bazelisk is installed and natively
# otherwise (scripts/run.sh says which); each has a `-native` twin. See
# docs/bazel.md.

# Build clax (Bazel), or the Rust workspace (native)
build *ARGS:
    ./scripts/run.sh build {{ARGS}}

# Build clax with Cargo
build-native *ARGS:
    CLAX_BUILD=native ./scripts/run.sh build {{ARGS}}

# Build the web UI and the extension into web/dist and web/dist-extension
web *ARGS:
    ./scripts/run.sh web {{ARGS}}

# Build the web UI and the extension with pnpm
web-native *ARGS:
    CLAX_BUILD=native ./scripts/run.sh web {{ARGS}}

# Run the web lint, typecheck, unit tests and bundle gate
web-test *ARGS:
    ./scripts/run.sh web-test {{ARGS}}

# Run the web lint, typecheck and unit tests with pnpm
web-test-native *ARGS:
    CLAX_BUILD=native ./scripts/run.sh web-test {{ARGS}}

# Run the Playwright end-to-end tests
web-e2e *ARGS:
    ./scripts/run.sh web-e2e {{ARGS}}

# Build the frontend and run the Playwright end-to-end tests with pnpm
web-e2e-native *ARGS:
    CLAX_BUILD=native ./scripts/run.sh web-e2e {{ARGS}}

# Run the Pi extension tests
pi-test *ARGS:
    ./scripts/run.sh pi-test {{ARGS}}

# Run the Pi extension tests with pnpm
pi-test-native *ARGS:
    CLAX_BUILD=native ./scripts/run.sh pi-test {{ARGS}}

# Run the Rust workspace tests
test *ARGS:
    ./scripts/run.sh test {{ARGS}}

# Run the Rust workspace tests with Cargo (the shell tests need `just web` once first); cargo nextest when installed
test-native *ARGS:
    CLAX_BUILD=native ./scripts/run.sh test {{ARGS}}

# Check formatting, run clippy with warnings denied, and the web lint
lint:
    ./scripts/run.sh lint

# Run clippy with warnings denied, with and without the test targets, plus the web lint, natively
lint-native:
    CLAX_BUILD=native ./scripts/run.sh lint

# Auto-format then run every check (dev loop)
check *ARGS:
    ./scripts/run.sh check {{ARGS}}

# Auto-format then run every quality gate natively
check-native *GATES:
    CLAX_BUILD=native ./scripts/run.sh check {{GATES}}

# Run every check CI runs
ci *ARGS:
    ./scripts/run.sh ci {{ARGS}}

# Run the native quality gates (scripts/quality_gates.sh)
ci-native *GATES:
    CLAX_BUILD=native ./scripts/run.sh ci {{GATES}}

# Run the perf gates' full versions (the checks run their quick versions): daemon latency, realtime clients, time to usable
perf *ARGS:
    ./scripts/run.sh perf {{ARGS}}

# Run the perf gates' full versions natively
perf-native:
    CLAX_BUILD=native ./scripts/run.sh perf

# Regenerate what Bazel derives from Cargo (after any Cargo.toml or Cargo.lock change); commit the result
bazel-sync:
    ./scripts/bazel-sync.sh

# Free what Bazel leaves behind: output bases of removed worktrees, and the shared cache beyond its bound (--dry-run, --report)
bazel-gc *ARGS:
    ./scripts/bazel-gc.sh {{ARGS}}
