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

# Check clax-core, clax-rt, clax-mcp, clax-server and clax-content-git for wasm32-unknown-unknown (a hosted Worker's target), warnings denied; needs an LLVM clang with a wasm32 backend (brew install llvm, or $LLVM_BIN)
check-wasm *ARGS:
    ./scripts/check-wasm.sh {{ARGS}}

# Run crates/clax-worker's checks natively (its own workspace, outside Bazel): npm ci, cargo fmt, its tests, clippy, and test-wasm.mjs under `wrangler dev --local`
worker-test:
    ./scripts/npm-ci-stamped.sh crates/clax-worker --silent
    cd crates/clax-worker && cargo fmt --all -- --check
    cd crates/clax-worker && cargo test -q --workspace
    cd crates/clax-worker && cargo clippy -q --workspace --all-targets -- -D warnings
    cd crates/clax-worker && node test-wasm.mjs

# Format the Rust code (the web lint is not a formatter)
fmt:
    cargo fmt --all

# Install clax from this checkout into $CARGO_HOME/bin (~/.cargo/bin), built with Bazel (natively without it), stop the agents' daemon if it runs that binary (the next agent call starts the new build), register the plugins with each harness found, and point the plugins at that binary (clax init sets the bin setting)
install:
    ./scripts/run.sh install
    . scripts/dev-home.sh && stop_installed_daemon "${CLAX_HOME:-$HOME/.clax}" "${CARGO_HOME:-$HOME/.cargo}/bin/clax"
    "${CARGO_HOME:-$HOME/.cargo}/bin/clax" init

# Install clax from this checkout, built natively (Cargo and pnpm)
install-native:
    CLAX_BUILD=native ./scripts/run.sh install
    . scripts/dev-home.sh && stop_installed_daemon "${CLAX_HOME:-$HOME/.clax}" "${CARGO_HOME:-$HOME/.cargo}/bin/clax"
    "${CARGO_HOME:-$HOME/.cargo}/bin/clax" init

# Remove the plugin registrations and the bin setting naming the installed clax (clax uninit), stop the agents' daemon if it runs that clax, and remove it
uninstall:
    -"${CARGO_HOME:-$HOME/.cargo}/bin/clax" uninit
    -. scripts/dev-home.sh && stop_installed_daemon "${CLAX_HOME:-$HOME/.clax}" "${CARGO_HOME:-$HOME/.cargo}/bin/clax"
    -cargo uninstall --root "${CARGO_HOME:-$HOME/.cargo}" clax-cli 2>/dev/null
    rm -f "${CARGO_HOME:-$HOME/.cargo}/bin/clax"

# Run the dev daemon in the foreground on ~/.clax-dev, port 7481 (extra args go to `clax serve`)
serve *ARGS:
    CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}" ./scripts/run.sh clax serve --foreground --port 7481 {{ARGS}}

# Stop the dev daemon (~/.clax-dev)
stop:
    CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}" ./scripts/run.sh clax stop

# Check the dev home and its daemon (~/.clax-dev)
doctor:
    CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}" ./scripts/run.sh clax doctor

# The Clax app (apps/clax_app). With Bazel each builds or checks from the
# shared cache (docs/bazel.md, "The Clax app"); each has a `-native` twin
# (Flutter, Gradle, Xcode and Cargo, as before).

# Build the Clax app's embedded clax: with Bazel for both Android CPUs and macOS, printing their paths; natively placed in apps/clax_app: [android|macos|all] [--debug]
[positional-arguments]
app-clax *ARGS:
    ./scripts/run.sh app-clax "$@"

# Build the Clax app's embedded clax natively (Cargo and the NDK) and place it in apps/clax_app
[positional-arguments]
app-clax-native *ARGS:
    CLAX_BUILD=native ./scripts/run.sh app-clax "$@"

# Run the Clax app's checks: Dart analysis, its tests, the prompt copies, its APK and macOS app's contents
app-test *ARGS:
    ./scripts/run.sh app-test {{ARGS}}

# Run the Clax app's checks natively (flutter analyze and test) against a kept debug clax
app-test-native:
    CLAX_BUILD=native ./scripts/run.sh app-test

# Run the Clax app's macOS integration tests, one app per file; each opens the app's window (--ios: the simulator ones, on "Clax iPhone")
app-integration *ARGS:
    ./scripts/run.sh app-integration {{ARGS}}

# Run the Clax app's macOS integration tests natively (flutter test -d macos)
app-integration-native:
    CLAX_BUILD=native ./scripts/run.sh app-integration

# Build each eval prompt into an app with the in-app agent on a live model, results in target/flutter-app/eval/<run>/ (needs ANTHROPIC_API_KEY; up to $6 a build, refused over --max-spend, $30 by default): [--model ID] [--prompts a,b] [--max-spend USD] [--run NAME]
[positional-arguments]
app-eval *ARGS:
    apps/clax_app/tool/eval/run.sh "$@"

# Build the Clax app's release APK (arm64 and x86_64, the ABIs clax is built for) and print its path
app-apk:
    ./scripts/run.sh app-apk

# Build the Clax app's release APK natively (Gradle)
app-apk-native:
    CLAX_BUILD=native ./scripts/run.sh app-apk

# Build HEAD's release APK and install it on the plugged-in phone, keeping the app's data
app-phone:
    ./scripts/app-phone.sh --i-know-what-i-am-doing

# Build HEAD's release APK natively (Gradle, in a clean copy) and install it on the plugged-in phone
app-phone-native:
    CLAX_BUILD=native ./scripts/app-phone.sh --i-know-what-i-am-doing

# Build the Clax app for the iOS simulator and run it on "Clax iPhone" (APP_IOS_DEVICE=ipad for "Clax iPad"), keeping its data
app-ios:
    ./scripts/app-ios.sh --i-know-what-i-am-doing

# Build HEAD's iOS app signed for a device and install it on the plugged-in or paired iPhone or iPad (APP_IPHONE_DEVICE picks one), keeping its data; --testflight uploads instead
[positional-arguments]
app-iphone *ARGS:
    ./scripts/app-iphone.sh --i-know-what-i-am-doing "$@"

# Build HEAD's iOS app signed for the App Store and upload it to TestFlight, with the next build number of ~/.config/clax/apple/builds
app-iphone-testflight:
    ./scripts/app-iphone.sh --testflight --i-know-what-i-am-doing

# Build the home app's bundle (apps/clax_app/home), as every app build does, and print its path
home-bundle:
    ./scripts/run.sh home-bundle

# Pack the home app's bundle natively (dart run) into apps/clax_app/assets/home
home-bundle-native:
    CLAX_BUILD=native ./scripts/run.sh home-bundle

# Build the Clax app's macOS app and print its path
app-macos:
    ./scripts/run.sh app-macos

# Build the Clax app's macOS app natively (Xcode)
app-macos-native:
    CLAX_BUILD=native ./scripts/run.sh app-macos

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

# Write rust-project.json from Bazel, so rust-analyzer needs no Cargo build (docs/bazel.md, "rust-analyzer")
rust-project:
    ./scripts/run.sh rust-project

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

# Run the perf benchmarks on demand (no check runs them, and they fail on no budget): daemon latency, realtime clients, time to usable
perf *ARGS:
    ./scripts/run.sh perf {{ARGS}}

# Run the perf benchmarks natively
perf-native:
    CLAX_BUILD=native ./scripts/run.sh perf

# Regenerate what Bazel derives from Cargo (after any Cargo.toml or Cargo.lock change); commit the result
bazel-sync:
    ./scripts/bazel-sync.sh

# Free what Bazel leaves behind: output bases of removed worktrees, and the shared cache beyond its bound (--dry-run, --report)
bazel-gc *ARGS:
    ./scripts/bazel-gc.sh {{ARGS}}

# Assemble the `clax` crates.io package in target/crate-package/clax (scripts/crate-package.py, after building the web UI), pack it with `cargo package` and check the packed crate with `cargo check --locked` (no binary is written, so target/debug/clax is left alone); nothing is uploaded
crate-package: web
    ./scripts/crate-package.py
    cd target/crate-package/clax && cargo package --allow-dirty --no-verify --target-dir ../pack
    rm -rf target/crate-package/unpacked && mkdir target/crate-package/unpacked
    tar -xzf target/crate-package/pack/package/clax-*.crate -C target/crate-package/unpacked
    # The packed manifest has no [workspace] table (cargo strips it); unpacked inside this repository it needs one.
    cd target/crate-package/unpacked/clax-* && printf '\n[workspace]\n' >> Cargo.toml && CARGO_TARGET_DIR="{{justfile_directory()}}/target" cargo check --locked

# Run the hosted demo locally in one go (demo/local.ts): build the web UI and the Worker, a fresh `wrangler dev --local` in hosted mode on a free port with one live seat, seed, links, steps and a git clone command; Enter starts the demo agents (extra args: --native for the native daemon instead, --port, --seats, --agents, --pace, --no-build)
[positional-arguments]
hosted-demo *ARGS:
    node demo/local.ts "$@"

# Deploy hosted Clax to the Cloudflare account `npx wrangler login` signed in to (crates/clax-worker/deploy.mjs): bucket, queue, Artifacts namespace, secrets, both hosts as custom domains, build, deploy, wait for /healthz (args: --zone, --public, --content, --store artifacts|memory, --dry-run, --diag, --no-build, --rotate-secrets)
[positional-arguments]
hosted-deploy *ARGS:
    cd crates/clax-worker && node deploy.mjs "$@"
