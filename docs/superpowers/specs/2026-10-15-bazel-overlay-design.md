# Bazel as an optional overlay over Cargo and npm

Date: 2026-10-15
Status: proposed; design only, nothing is converted yet

Owner decision (2026-10-15): Clax moves its builds and tests to Bazel, as an
optional overlay over the native tools, the way `../changelist` does
(its `docs/bazel.md`).

## 1. Problem and goal

Every agent worktree builds Clax from nothing into a `target/` of its own.
Measured on the owner's machine on 2026-10-15: three worktrees' `target/`
directories held 16 GB, 13 GB and 9.9 GB, the main checkout's 2.5 GB, and the
data volume was 86% full (133 GiB free of 926). Removing a worktree is the
only thing that frees its `target/`, and the next worktree pays for the
whole build again.

Goal: worktrees share one compile cache, so a new worktree at a commit
another has built runs every check without compiling, without `npm ci`, and
with almost no disk of its own; while nothing that works today stops working
without Bazel.

### Goals

- One compile cache for every worktree, with stated disk budgets and a
  command that enforces them.
- The whole check suite stays under two minutes on a warm cache, under Bazel
  as under the native tools.
- Bazel when it is installed, the native tools otherwise, and every run says
  which one ran.
- macOS's first-run assessment of new executables never lands inside a
  test's time limit, under Bazel as today.
- Cargo, `Cargo.lock`, `package.json` and the lockfiles stay the source of
  truth; Bazel derives from them and a check fails when the derived files
  are stale.
- Lands in phases (Rust, then web, then CI), each shippable, without
  blocking the multi-person branch or anything else merging into main.

### Non-goals

- Replacing the dev loop: `just dev`, `just watch`, `just serve` and
  rust-analyzer stay on Cargo and Vite.
- Building releases with Bazel: `release.yml` keeps Cargo (§11).
- A remote cache or remote execution.

## 2. What changelist does, and where Clax differs

Changelist's setup, read in full for this design (`docs/bazel.md`,
`MODULE.bazel`, `.bazelrc`, `bazel/`, `scripts/run.sh`,
`scripts/lib/build-system.sh`, `scripts/bazel-test.sh`, `bazel-sync.sh`,
`bazel-gc.sh`, the `justfile`, `.github/workflows/ci.yml`):

| Changelist | Clax |
|---|---|
| `scripts/run.sh` picks Bazel when Bazelisk (or a `bazel` at `.bazelversion`) is on `PATH`, else the native tools, and says so before and after; `CHANGELIST_BUILD=bazel\|native\|auto` overrides; every recipe has a `-native` twin | The same, with `CLAX_BUILD` (§4) |
| One disk cache, `~/.cache/bazel-disk`, 20 GB and 7 days, trimmed by Bazel when idle and by `scripts/bazel-gc.sh` | Its own cache with its own budget (§9, Q2) |
| Third-party crates from `Cargo.lock` through crate_universe; `bazel/workspace.bzl` derived from `cargo metadata`; `scripts/bazel-sync.sh --check` fails on drift | The same; Clax also derives `[workspace.lints]` (changelist has none) and the test-only features of its own crates (§5) |
| `cargo_package()` globs `tests/*.rs`, one test target each | Clax's crates set `autotests = false` and name one or two test binaries each; targets come from the `[[test]]` entries (§5.3) |
| Integration tests that run the server binary compile outside the sandbox and outside the cache (`no-sandbox`, `no-cache`) for an absolute `CARGO_BIN_EXE_` | Clax's tests take the binary at run time (`CLAX_TEST_BIN`), so they stay sandboxed and cached (§6) |
| No browser tests; Postgres as an optional pass | Playwright, the Chrome extension's browser tests, the real Pi, and three perf gates (§8) |
| pnpm; rules_js reads `pnpm-lock.yaml` | npm today; the lockfile question is Q1 (§7) |
| Lints and format through rules_rust's clippy and rustfmt aspects | The same |
| CI gates on Bazel on Blacksmith runners with a sticky disk per job; the native checks are a manual workflow | Clax's CI runs on GitHub-hosted runners today; runner and cache are Q3 |
| Deploys still build with Cargo; a Bazel image build exists but is off | Releases keep Cargo (§11) |
| Nothing about macOS first-run assessment | Central for Clax (§6) |

## 3. Decisions

| # | Decision | Reason |
|---|---|---|
| B1 | Bazel is an overlay: every check and build keeps a native path that works without Bazel, and `just` recipes run Bazel when it is installed. | Owner decision (2026-10-15). A person or agent without Bazel loses nothing; one with it gains the shared cache. |
| B2 | One switch, `CLAX_BUILD=bazel\|native\|auto` (default `auto`), read by one script, `scripts/run.sh`. `bazel` fails with the install command rather than fall back; `native` never looks for Bazel; `auto` uses Bazel when Bazelisk, or a `bazel` at `.bazelversion`'s version, is on `PATH`. | Changelist's model, proven there. A silent fallback where Bazel was required would hide a real difference between what ran and what CI checks. |
| B3 | Every native run prints, as its last line, that it ran with the native tools and that CI's result can differ. A run that fell back because Bazel was missing also says so first, with the install command. Neither is an error; the exit status is the checks' own. | Agents report results; they must not report a native pass as the gate CI runs (once CI gates on Bazel, §14). |
| B4 | Bazel, its rules and their versions are changelist's: Bazel 9.3.0 (`.bazelversion`), rules_rust 0.74.0, aspect_rules_js 3.5.1, rules_nodejs 6.7.6, bazel_lib 3.7.2, rules_shell 0.8.0. They move together. | One Bazel install and one set of known rule behaviours on the machine. Changelist's notes on these versions (crate_universe's feature merging, rules_js's link guard) apply as written. |
| B5 | The Rust toolchain in `MODULE.bazel` is `rust-toolchain.toml`'s (1.94.0), with checksums for each archive, and `scripts/bazel-sync.sh --check` fails when the two differ. | Bazel fetches its own compiler; with checksums the unpacked toolchain is shared between output bases rather than unpacked per worktree. |
| B6 | Cargo is the source of truth. `bazel/workspace.bzl` (each crate's targets, features, path dependencies, test targets and test-only features) and `MODULE.bazel.lock` are derived and committed; `just bazel-sync` rewrites them and `--check` fails when stale. BUILD files glob sources, so a new source or test module needs nothing. | Changelist's rule. A hand-kept second description of the build would drift. |
| B7 | `[workspace.lints]` is derived too: every workspace crate is compiled under Bazel with the flags Cargo derives from it (today `-F unsafe_code`), and the drift check covers it. | Cargo applies `unsafe_code = "forbid"` from `Cargo.toml`; rules_rust does not read `Cargo.toml`. Without this, a Bazel-only pass would accept `unsafe`. Changelist has no workspace lints, so this part is new. |
| B8 | Test targets come from Cargo's `[[test]]` entries (each crate's `tests/integration.rs`, and `clax-cli`'s `descriptors.rs` and `clax-server`'s `web_dist.rs`), with every file under `tests/` as a source of each. | Clax sets `autotests = false`: each file in `tests/` is a module of one binary, not a binary. Changelist's glob would make 50 test binaries in `clax-server`. |
| B9 | A crate's tests link a variant of the workspace crates built with the features its dev-dependencies ask for (`clax-server`'s `test-routes` and `test-support`), derived from `cargo metadata`. The libraries and binaries everything else uses have only their default features. | This is what Cargo does per package; it also makes clippy over the plain targets the "without test-only features" pass that `quality_gates.sh` runs today. |
| B10 | Compile-time embeds name their directory by a variable with a default (rust-embed's `interpolate-folder-path`): `web/dist`, `web/dist-extension` and the plugin trees. Cargo uses the defaults, the source paths of today; Bazel points them at its own outputs. | rust-embed resolves `folder` against `CARGO_MANIFEST_DIR` at compile time. Under Bazel the web build is a generated directory under `bazel-out/`, not `web/dist`, so a fixed relative path names the wrong (empty) directory. |
| B11 | Bazel's debug builds embed the web UI, the extension and the plugins, as a release build does: rust-embed's `debug-embed` in Bazel's crate repository, and one cfg (`clax_embedded`) under which Clax's own debug-only disk reads (`routes/shell.rs`'s `web_dist()` and the debug `bridge_version`) use the embedded files. `set_web_dist`, the override tests use, is unchanged. | A Cargo debug build reads `web/dist` and the plugins from the absolute source path at run time, which is what `just dev` relies on. A Bazel binary must not name a path in one sandbox or one worktree: it is shared through the cache and run from a test's own directory. |
| B12 | `CARGO_MANIFEST_DIR` is the crate's path from the workspace root, as changelist sets it, and tests run from the runfiles root. Tests make a repository path absolute (`std::path::absolute`) before handing it to a child process or another directory; under Cargo, where the path is already absolute, that changes nothing. | An absolute compile-time path differs per sandbox and would keep compiled tests out of the shared cache. No `std::env::set_var` (it is `unsafe` in edition 2024): paths reach children through `Command::env`. |
| B13 | Under Bazel, `CLAX_BUILD_COMMIT` is `unknown` for every build: the build script never runs `git`. | A build action that reads `.git` is not reproducible, and every commit would rebuild `clax-cli`. `crates/clax-cli/tests/version.rs` already expects the build's own value. Releases, built by Cargo, keep the real commit. |
| B14 | Tests get the `clax` binary at run time: `CLAX_TEST_BIN` and `CARGO_BIN_EXE_clax` name the Bazel-built `clax` in the test's runfiles, and the tests' one helper makes it absolute at first use. Every `Command::cargo_bin("clax")` goes through that helper. | Changelist's `CARGO_BIN_EXE_` route compiles each such test outside the sandbox and the cache. Clax's tests already take the binary from `CLAX_TEST_BIN`; only the path's form changes. |
| B15 | macOS first-run assessment: each Bazel-built `clax` is run once by a warm-up action (§6) before any test can start, and tests depend on its stamp. Test helper programs keep their content-keyed store across runs through `CLAX_FAKE_EXE_DIR` (§6.3). | The assessment belongs to the file. Bazel makes a new file per output base per build; the warm-up pays for it once, outside every test's limits, as `scripts/stable-bin.sh` does today. |
| B16 | The web side builds and tests under rules_js, as changelist's does: each project's `package.json` scripts run over the whole project, with Node and packages Bazel fetches by the lockfile's checksums. Nothing is restated: a target names a script. | Changelist's runner (`bazel/js/npm_script.mjs`) and macros (`bazel/js/defs.bzl`) already solve Vite under Bazel, the copy-not-link question and stale-file safety. Granularity per project needs no per-file BUILD upkeep. |
| B17 | The web build (`//web:dist`, which writes `dist`, `dist-extension` and `dist-extension-test`) and Pi's packages (`//plugins/pi:node_modules`) are part of the Rust phase, because the Rust tests read them. | `clax-server`'s shell tests read the built web UI; the real-Pi tests (the canary sweep on the multi-person branch, `crates/clax-server/tests/common/clients.rs` there) run `plugins/pi/node_modules/@mariozechner/pi-coding-agent/dist/cli.js`. A Rust phase without them would leave those tests native-only. |
| B18 | Which tests are Bazel targets, and which stay native with a reason, is §8's table. Browser tests and perf gates are Bazel targets; tests that need a real harness CLI, a model or credentials stay native. | Clax's daemons, browsers and Pi runs already use scratch homes and port 0; nothing in them reads the person's machine. Real harness sessions do. |
| B19 | Disk: Clax's own disk cache, `~/.cache/clax-bazel-disk`, trimmed to 30 GB and 7 days; per-worktree real disk under Bazel below 0.5 GB for a worktree that builds main and below 3 GB after edits; `just bazel-gc` removes output bases of removed worktrees (§9). | Measured `target/` sizes are 10–16 GB per worktree. The budgets are what Phase 1 must show before Bazel becomes the default for a task. |
| B20 | Speed: on a warm cache, a Bazel task takes no longer than its native twin, and the whole suite stays under two minutes. A recipe defaults to Bazel only once Phase 1's (or 2's) measurements show that (§10). | The owner's rule: the check suite stays under two minutes. Disk savings do not pay for a slower loop (Q5 if the two conflict). |
| B21 | The dev loop stays native: `just dev`, `just watch`, `just serve`, `just stop`, `just doctor`, `just install`, `just demo-room-sample`, and rust-analyzer. | Cargo's incremental compilation and cargo-watch, Vite's rebuilds, and a debug binary reading `web/dist` from disk are what the loop is built on. |
| B22 | Releases build with Cargo, unchanged; `scripts/test-release.sh` becomes a Bazel test (§11). | Static musl binaries on four native runners, the real `CLAX_BUILD_COMMIT`, and a release that reads no build state anything else wrote. |
| B23 | No `unsafe` and no hard links stay true under Bazel, and both are checked (§13). | Owner rules. |
| B24 | Phases: 1 Rust (with the two inputs of B17), 2a web, Pi and script gates, 2b browsers and perf, 3 CI. Each is shippable; CI changes only in phase 3, so main's gate stays `scripts/quality_gates.sh` until then (§15). | Bazel must not block the multi-person branch or anything else merging into main. |

## 4. The overlay

### 4.1 The switch and the entry point

`scripts/lib/build-system.sh` (sourced) chooses once, by what is installed:

| On `PATH` | `CLAX_BUILD=auto` runs |
|---|---|
| Bazelisk, or a `bazel` at `.bazelversion`'s version | Bazel |
| Neither (a `bazel` of another version is not used) | The native tools: Cargo, cargo-nextest when installed, Node and npm (or pnpm, Q1) |
| Neither Bazel nor the native tools | Nothing; one message names both ways to install |

`scripts/run.sh <task>` is the only caller. `scripts/run.sh which` says what
`auto` picks here and why; `--dry-run <task>` prints the commands without
running them. `scripts/run.test.sh` tests the choice on a `PATH` of fake
tools (made with `scripts/fake-exe.sh`), in both gates.

CI does not go through `run.sh` once it gates on Bazel (§14): it calls
`scripts/bazel-test.sh`, so a runner without Bazel fails and never falls
back.

### 4.2 Recipes

| Recipe | With Bazel | Native twin (`CLAX_BUILD=native`) |
|---|---|---|
| `just ci` | `scripts/bazel-test.sh --ci` | `just ci-native`: `scripts/quality_gates.sh` |
| `just check` | `cargo fmt`, then `bazel-test.sh --ci` | `just check-native`: `scripts/check.sh` |
| `just test` | the Rust tests (`bazel-test.sh --rust`) | `just test-native`: today's recipe (nextest with `CLAX_TEST_BIN`) |
| `just lint` | format check, clippy aspect, web lint | `just lint-native` |
| `just web-test` | `//web:lint`, `//web:typecheck`, `//web:unit` | `just web-test-native` |
| `just pi-test` | `//plugins/pi:typecheck`, `//plugins/pi:test` | `just pi-test-native` |
| `just web-e2e` | `//web:e2e_*` (§8) | `just web-e2e-native` |
| `just perf` | the full perf targets (§8) | `just perf-native` |
| `just build` | `bazelisk build //crates/clax-cli:clax` | `just build-native`: `cargo build --workspace` |
| `just web` | `//web:dist`, then its files copied into `web/dist` and `web/dist-extension` (copies, never links) | `just web-native`: today's recipe |
| `just bazel-sync`, `just bazel-gc` | new (§5.2, §9) | — |

`just web` with Bazel exists for the dev loop's first start: a fresh
worktree takes the web build from the cache instead of running `npm ci` and
Vite. The dev loop itself (B21) has no Bazel path.

A recipe moves to Bazel in the phase that ports its checks, and only after
that phase's measurements (§10, §15). Until then it is native whether or not
Bazel is installed, and the overlay says nothing.

## 5. Rust under Bazel

### 5.1 Files

| File | Holds | Kept by |
|---|---|---|
| `.bazelversion` | Bazel 9.3.0 | hand (B4) |
| `.bazelrc` | shared settings (§5.5, §9, §10) | hand; `--check` fails if it limits jobs or cores |
| `user.bazelrc.example` | per-machine limits and paths, to copy to the git-ignored `user.bazelrc` or `~/.bazelrc` | hand |
| `.bazelignore` | `.claude` (the main checkout holds every worktree under `.claude/worktrees/`), `target`, `node_modules` everywhere, `web/dist*`, `dist`, `.superpowers` | hand |
| `MODULE.bazel` | toolchains, crate_universe over the workspace's manifests and `Cargo.lock`, the npm repositories (§7), Playwright's browser (§8) | hand, with the toolchain checked (B5) |
| `MODULE.bazel.lock` | pinned crates and modules | derived |
| `bazel/cargo.bzl` | `cargo_package()`, Clax's version of changelist's macro | hand |
| `bazel/workspace.bzl` | per crate: version, targets, `[[test]]` entries, default features, dev-dependency features of workspace crates, path dependencies, workspace lint flags | derived |
| `crates/*/BUILD.bazel` | one `cargo_package()` call each, plus the files a crate reads outside `src/` | hand |
| `scripts/bazel-sync.sh`, `scripts/bazel-test.sh`, `scripts/bazel-gc.sh` (and tests) | §5.2, §8, §9 | hand |

`/bazel-*` and `/user.bazelrc` are git-ignored.

### 5.2 Keeping Bazel in step with Cargo

Most changes need nothing: a new source file, a new module under `tests/`,
or a new use of a crate already in `Cargo.toml`. A change to any
`Cargo.toml` or `Cargo.lock` needs `just bazel-sync` committed with it.
Until phase 3 a stale sync breaks only Bazel runs (the overlay names the
command); from phase 3, CI fails on it. `bazel-test.sh` and CI never rewrite
derived files. A merge conflict in a derived file is resolved by running the
sync on the merged manifests, never by hand.

A new crate needs a `BUILD.bazel` calling `cargo_package()`, its manifest
listed in `MODULE.bazel`, and the sync. The sync fails when a workspace
member has no `BUILD.bazel`, so a branch adding a crate learns it at its
first Bazel run.

As in changelist, `scripts/bazel-sync.sh --check` compares what Bazel
resolved with `cargo tree` (per host, with and without dev-dependencies) and
fails on any difference not listed with its reason in an allowlist
(changelist's `NATIVE_EXTRA`). Clax's list is not known until the first
sync; §16 explains why it is not empty.

### 5.3 `cargo_package()` for Clax

Per crate, with `<lib>` the crate name with underscores:

| Target | Is |
|---|---|
| `<lib>` | the library, default features |
| `<lib>_test` | the variant other crates' tests and its own link, with the features dev-dependencies ask for (B9); only for crates that have such features (`clax_server`) |
| `<lib>_lib_test`, `<lib>_doc_test` | unit tests and doc tests |
| `clax` | the binary (`clax-cli`) |
| `clax_warm` | its warm-up stamp (§6) |
| `<test>` | each `[[test]]` entry: `integration`, and `descriptors` and `web_dist` where they exist |

Arguments a BUILD file sets by hand: `compile_data` (files outside `src/` a
crate compiles in, such as `web/extension/key/key.pub.b64` for
`clax-core`'s build script and the plugin trees for `clax-cli`),
`test_data` (fixtures, `web/contract/`, the scripts `clax-cli`'s tests run),
and `tests_use_clax` (tests that run the binary: adds `:clax` and
`:clax_warm` as data and sets `CLAX_TEST_BIN` and `CARGO_BIN_EXE_clax`).

Build scripts run as rules_rust build-script targets with their inputs
declared. `clax-cli`'s script still writes `plugin-dirs.txt`; under Bazel it
lists the declared plugin directories. Bazel sets `CLAX_BUILD_COMMIT=unknown`
for it (B13), so it never runs `git`.

`rustc_env` sets `CARGO` to a path that does not exist: the tests' fallback
that runs `cargo build` when `CLAX_TEST_BIN` is unset must fail loudly under
Bazel rather than build into the sandbox, and it is never reached because
Bazel always sets `CLAX_TEST_BIN`.

### 5.4 Test sharding

`clax-server`'s `integration` binary holds about 50 modules. Under nextest
each test is a process; under Bazel a test target is one process per shard.
Large targets get a `shard_count`, and within a shard libtest runs tests on
threads, as `cargo test` does. If rules_rust's sharding is unavailable for
these targets, a small wrapper lists the tests (`--list`) and runs its shard's
share with `--exact`. The two process-sensitive suites are already separate
binaries (`descriptors`, `web_dist`).

### 5.5 Lints and format

`--config=fmt` runs rules_rust's rustfmt aspect (same rustfmt as the
toolchain); `--config=clippy` the clippy aspect with `-D warnings` and the
workspace's one `clippy.toml`. Over every target it is today's
`--all-targets` pass; over the libraries and binaries alone (B9) it is the
"no test features" pass. The `no unsafe code`, `no hard links` and
`pipefail pipes` scans become `sh_test`s over the declared sources (§13).

### 5.6 Test environment

`.bazelrc` sets `--incompatible_strict_action_env`, so actions and tests see
`PATH=/bin:/usr/bin:/usr/local/bin`: `git`, `bash`, `perl`, `python3` and
`shasum` come from the system, and Homebrew's tools are absent. A test that
needs another tool gets it from Bazel: Node from the Node toolchain (named by
`CLAX_TEST_NODE` where a Rust test runs Node, as the real-Pi driver does),
Chromium from its pinned repository (§8). A test that needs Homebrew's `just`
(`scripts/test-justfile.sh`) runs outside Bazel (§8).

`test --test_env=RUST_TEST_THREADS` and `TOKIO_WORKER_THREADS` are set as in
changelist; values come from Phase 1's measurements.

## 6. Executables and macOS first-run assessment

### 6.1 The facts this rests on

From `crates/clax-fake-exe/src/lib.rs`, `scripts/fake-exe.sh` and
`scripts/stable-bin.sh`: macOS assesses every new executable file on its
first run, one at a time (syspolicyd, XProtect); on a loaded machine that
run can take seconds to minutes. The assessment belongs to the file, not its
bytes or its name: a new file with the same bytes is assessed again, and
running an assessed file through a symbolic link is not a first run.

What Bazel does with executables:

- A built binary is a file in the worktree's output base. A disk-cache hit
  materializes it as a new file in that output base, so each worktree has
  its own file per build.
- A test's runfiles reach it through symbolic links, and the sandbox exposes
  inputs as symbolic links, so every test in one output base runs the same
  file.
- External repositories fetched with checksums (the toolchains, Node, the
  pinned Chromium) are unpacked once into a store shared by every output
  base, so each is one file per machine.

So, without help, the first test to run a newly built `clax` pays the
assessment inside its own limits, and every test started meanwhile waits
behind it; and every new test binary is assessed when Bazel first runs it,
as under Cargo today ("On macOS every new test binary is assessed on its
first run", `docs/usage.md`).

### 6.2 The warm-up action

`clax_warm` is an action whose input is the `clax` binary and whose output is
a stamp. It runs `clax --version` once, which makes the assessment, and
writes the binary's SHA-256 to the stamp. It is tagged `no-remote-cache`, so
a disk-cache hit can never stand in for it in a worktree whose file has not
been run; the output base's own action cache still skips it while the binary
is unchanged. Every test that runs `clax` (B14) has the stamp as data, so no
such test starts before the binary has been run once.

This is `scripts/stable-bin.sh`'s rule (pay the first run once per build,
before any caller), with Bazel's output base as the store: the file the test
runs is the file the warm-up ran.

The test binaries themselves are assessed on first run, before `main`, so
inside Bazel's test timeout (which is long) and outside any limit a test
sets. That is today's behaviour.

The `just dev` path is unchanged: it copies a Cargo build and runs it.

### 6.3 Test helper programs

`clax-fake-exe`, `scripts/fake-exe.sh` and `plugins/pi/test/fake-exe.ts`
keep each script text once, read-only, under a hash of the text, in
`$TMPDIR/clax-fake-exe`, and run it once before a test relies on it. Under
Bazel each test has a temporary directory of its own, so that store would be
new for every test run and every fake would be a first run every time.

All three take the store from `CLAX_FAKE_EXE_DIR` when it is set.
`scripts/bazel-test.sh` and `scripts/run.sh` pass
`--test_env=CLAX_FAKE_EXE_DIR=<the native store's path>` and
`--sandbox_writable_path=<the same>`, so Bazel's tests and the native gates
share one store per machine. A plain `bazelisk test` without them is still
correct, only slower on a loaded machine. Sharing is safe: entries are keyed
by their text, read-only, and the warm-up run is idempotent, so a test's
result cannot depend on what an earlier one stored.

### 6.4 What phase 1 measures before relying on this

- A fresh `clax` under Bazel's macOS sandbox: one assessment, paid in
  `clax_warm`, and no test's first exec of `clax` over 1 s (logged by the
  helper).
- The same in a second worktree at the same commit: one more assessment
  (new file), still in `clax_warm`.
- A full test run with the machine loaded by another worktree's build: no
  timeout attributable to a first run.

If the per-worktree assessment is still too slow, Q4's other option keeps
one copy per content per machine.

## 7. The web side and Pi

### 7.1 rules_js, as changelist

Bazel runs `web/`'s and `plugins/pi/`'s own scripts with Node and packages
it fetches from the lockfile by checksum, through changelist's
`npm_script_test` and `npm_script_build` (`bazel/js/defs.bzl`) and its runner,
which copies exactly the declared files into a private directory, runs the
script, and takes the declared outputs (real files, not links, which Vite's
native bundler needs). Copied as they are, with their tests
(`scripts/bazel-web.test.sh`: a deleted module, test or package is really
gone).

rules_js reads `pnpm-lock.yaml`. Clax's projects use npm
(`package-lock.json`), so the lockfile is Q1: switch both projects to pnpm,
as changelist did, or keep npm with a `pnpm-lock.yaml` derived from each
`package-lock.json` and checked by `bazel-sync --check`. Q1 must be
answered before phase 1 starts, since the web build is part of it (B17).

Node is pinned in `MODULE.bazel` at an exact 22.x release, the major CI uses
(`actions/setup-node`, `node-version: 22`). `.bazelrc` sets `DO_NOT_TRACK`,
as changelist's does.

### 7.2 Targets

| `quality_gates.sh` gate | Bazel |
|---|---|
| web npm ci | `//web:node_modules`, `//plugins/pi:node_modules` (no install in the worktree) |
| web build | `//web:dist` (`npm run build`: `dist`, `dist-extension`, `dist-extension-test`) |
| web unit | `//web:unit` (`npm test`) |
| web lint | `//web:lint` (`npm run lint`, oxlint) |
| web typecheck | `//web:typecheck` |
| web bundle size | `//web:bundle_size` (`node scripts/bundle-size.mjs` over `//web:dist`) |
| pi npm ci, pi extension | `//plugins/pi:typecheck`, `//plugins/pi:test` (with `:clax` and `:clax_warm` as data, `CLAX_TEST_BIN` set) |
| web e2e, time to usable | §8 |

The bundle gate is a test over the build's output and
`web/perf/bundle-budget.json`; it stays exactly `bundle-size.mjs`. Its
`--record` mode writes the budget file in the source tree, so it is a native
command only.

The extension's two builds are outputs of `//web:dist`. `clax-cli` embeds
`dist-extension` through B10; the browser tests load `dist-extension-test`
from their runfiles. `scripts/build-web.sh`'s restoring of modification times
exists for Cargo's rebuild check and has no Bazel counterpart: Bazel goes by
content.

`plugins/pi/node_modules` in a Rust test's runfiles is rules_js's link tree
at that same path, so the real-Pi driver finds
`node_modules/@mariozechner/pi-coding-agent/dist/cli.js` where it looks
today.

## 8. Tests that start daemons, browsers and real agent clients

| Suite | Under Bazel | Tags and resources | Reason |
|---|---|---|---|
| Rust tests starting in-process or child daemons (`TestServer`, `clax serve --port 0`) | Bazel test (phase 1) | sandboxed; network allowed | Scratch homes and port 0 already; nothing reads the person's machine |
| Real-Pi and canary tests (`RealClients` on the multi-person branch: the MCP shim, hooks, `/mcp`, the real Pi with a scripted model) | Bazel test (phase 1) | data: `//plugins/pi:node_modules`, the Node toolchain, `:clax_warm` | Pi runs `--offline` with a scripted model; every input is a declared file |
| Pi extension tests (`plugins/pi/test`) | Bazel test (phase 2a) | as above | Same daemon fixture, scratch homes |
| Script gates: release scripts, installer, plugin wrapper, tool hook, extension signing, stable bin, dev scripts, comment loop, hard links, pipefail | `sh_test`s (phase 2a) with their files declared | fakes through `CLAX_FAKE_EXE_DIR` | Each runs in a scratch directory with fakes and a 127.0.0.1 server |
| Plugin structure (`scripts/test-plugins.sh`) | run by `bazel-test.sh`, not a Bazel test | — | It compares the pinned release with the newest `v*` git tag: it reads the repository's history, which is not a file Bazel can declare |
| `scripts/test-justfile.sh` | run by `bazel-test.sh`, not a Bazel test | — | It runs `just`, which is Homebrew's (§5.6) |
| Playwright browser tests (`web/e2e`) | Bazel tests (phase 2b), one per Playwright project (`gesture`, `chrome-overlay`, `rest`) | `no-sandbox`; a CPU reservation per target; Chromium from a pinned repository | Each worker starts its own daemon on port 0. Outside the sandbox because Chromium applies its own macOS sandbox profile and runs a GPU and helper process tree; phase 2b confirms whether the sandbox can stay on |
| Time to usable (`web/perf`) | Bazel test (phase 2b) | `exclusive`, `no-sandbox` | It measures latency, so nothing else may run beside it |
| Daemon latency, realtime clients (`scripts/perf-daemon.sh`, `perf-clients.sh`) | Bazel tests (phase 2b), quick mode in `--ci`, full mode `manual` for `just perf` | `exclusive`, `no-sandbox`; the release `clax` (`-c opt`) as data | Same; `exclusive` is today's "alone, one at a time" |
| `scripts/smoke-*.sh` (Claude, Codex, Grok, Pi, push, ask, capabilities, release binary) | native only, manual | — | Real harness CLIs, a model and the person's credentials |
| `scripts/verify-harnesses.sh` | native only, manual | — | The real `claude`, `codex` and `pi` on `PATH` |
| `scripts/demo-room-sample.sh` | native only | — | A demo, not a check |

Playwright's Chromium comes from a repository rule pinned by URL and
checksum per platform (the version `@playwright/test`'s locked release
names, written by `bazel-sync` into `bazel/playwright/browsers.bzl`), and the
browser tests set `PLAYWRIGHT_BROWSERS_PATH` to it. No test downloads a
browser or uses the machine's. On Linux CI the system libraries Chromium
needs are installed before Bazel runs (the one thing CI installs besides
Bazelisk), as `playwright install --with-deps` does today.

A failing test's result is never cached. A passing one is cached on its
inputs; for the perf gates that is the release binary, the scripts and the
budget files (Q6).

Bazel runs `exclusive` tests after the others, one at a time, which is the
order `quality_gates.sh` uses today. Tests in other worktrees still load the
machine, as today.

## 9. Disk

### 9.1 Where Bazel keeps files

- **The disk cache**, `~/.cache/clax-bazel-disk` (B19, Q2): every action's
  outputs, shared by every Clax worktree. `.bazelrc`:
  `--disk_cache=~/.cache/clax-bazel-disk`,
  `--experimental_disk_cache_gc_max_size=30G`,
  `--experimental_disk_cache_gc_max_age=7d`.
- **One output base per worktree** under Bazel's user root
  (`~/Library/Caches/bazel/_bazel_$USER/` on macOS). Its files come from the
  cache as clones, which `du` counts and the disk does not (changelist's
  measurement: nine edited worktrees added 60 MB each beyond the cache).
- **Downloads and unpacked toolchains**, shared by every output base of
  every repository: the Rust toolchain, crates, npm packages, Node, Chromium.
- **The fake-executable store** (§6.3), unchanged and small.

### 9.2 Budgets

| What | Budget | Enforced by |
|---|---|---|
| Disk cache | 30 GB, entries unused for 7 days removed | Bazel while idle; `scripts/bazel-gc.sh` at any time |
| A worktree at a commit some worktree has built | under 0.5 GB of real disk | Phase 1 measurement |
| A worktree after edits, all tests built | under 3 GB of real disk | Phase 1 measurement; naming one test target builds one test binary |
| Output bases of removed worktrees | 0 after `just bazel-gc` | `scripts/bazel-gc.sh` |
| `target/` in a worktree that only runs checks | none | Bazel writes none; only the dev loop and rust-analyzer make one |
| All of Clax's Bazel state on a machine with 20 worktrees | under 60 GB | `scripts/bazel-gc.sh --report` prints the real total |

The 30 GB figure is a starting point: phase 1 measures what one build of
main and one edit of each crate add to the cache and sets the bound to hold
about five edited worktrees beside main's build.

### 9.3 `just bazel-gc`

Changelist's `scripts/bazel-gc.sh`, adapted: removes output bases whose
workspace directory is gone, was inside this repository's checkouts, and
has no running server (`git worktree remove` does not delete them; this
machine held 62 output bases on 2026-10-15); trims the disk cache to the
bounds above without waiting for an idle server; refuses a bound that is not a whole
number above zero, and a cache directory other than `.bazelrc`'s unless
adopted; `--dry-run` first. It finds servers by the PID file Bazel writes in
each output base, never by `pgrep -f`. `--report` prints each part's real
size. Run it after removing worktrees.

Bazel's server per worktree exits after 15 idle minutes and its heap is
capped at 3 GB (`startup --max_idle_secs=900`, `--host_jvm_args=-Xmx3g`), as
in changelist.

## 10. Speed

### 10.1 Budgets

| Case | Budget |
|---|---|
| `just ci`, nothing changed since the last run in this worktree | under 20 s |
| `just ci` in a fresh worktree at a commit another worktree built | under 60 s |
| `just ci` after an edit (one crate, or `web/shell`) | under 2 min, and no slower than `just ci-native` after the same edit |
| `just test` after an edit to one crate | no slower than `just test-native` |

Measured on the owner's machine, idle, in phase 1 (Rust) and 2 (all), each
recorded in `docs/bazel.md` with the commands. A recipe switches its
default to Bazel only when its rows hold (B20).

### 10.2 Why an edit can be slower, and what is done

Bazel compiles each crate without rustc's incremental cache, and every test
binary links the whole crate graph. Changelist measured about 3 minutes for
its Rust half after a server edit. Clax's measures, in order of cost:

- pipelined compilation and 16 codegen units (`.bazelrc`), as changelist;
- the test-feature variant only where a crate has test features (B9), so
  most crates compile once;
- shards for the large integration binaries (§5.4);
- the clippy pass and the test build run in one `bazel` invocation, so they
  share the machine instead of queueing;
- no symbol stripping or debug-info change until Q7 is answered.

If an edit still runs past the budget, the task stays native by default
(Q5); the shared cache still serves every worktree that has not edited
Rust.

## 11. Releases

`release.yml` is unchanged: Cargo on four native runners, static musl on
Linux, `CLAX_BUILD_COMMIT` from the tag's commit, `just web` before the
build. Reasons: Bazel would need a musl C toolchain for `rusqlite`'s bundled
SQLite and for `ring` (on the multi-person branch) on each target; the
release's commit stamp is the opposite of B13; and a release must read no
build state that another build wrote, which a shared cache is.

`scripts/test-release.sh` tests the release scripts in a scratch copy of
the files they touch, with a fake release server on 127.0.0.1 and fake
executables. It becomes an `sh_test` whose data is a filegroup of exactly
those files (the list now in the script's `FILES`, which then reads the
filegroup's manifest instead), so a file the scripts start touching fails
the test until declared.

The perf gates' release `clax` is a Bazel `-c opt` build of
`//crates/clax-cli:clax`, used for measuring only; nothing ships it.

## 12. The dev loop and rust-analyzer

Native, unchanged (B21). `just dev` and `just watch` build with Cargo into
the worktree's `target/`, and a Cargo debug build reads `web/dist` from disk
so Vite's rebuilds show at the next reload. rust-analyzer reads the Cargo
workspace. A worktree used only for checks never has a `target/`.

`just web` with Bazel (§4.2) fills `web/dist` from the cache for a
fresh worktree's first `just dev`; after that the dev loop rebuilds it.

## 13. Invariants

**No `unsafe`.** Bazel compiles every workspace crate with the flags
derived from `[workspace.lints]` (B7), today `-F unsafe_code`; the drift
check fails when they differ. The source scan runs as an `sh_test`. The
enabling changes (§15.1) add no `unsafe`; in particular they pass paths to
children with `Command::env`, never `std::env::set_var`.

**No hard links.** Clax's Bazel files and scripts make none: the copy rules
that take a `hardlink` attribute (bazel_lib's) are given `"off"`, the runner
copies files, `just web` copies, and `scripts/check-no-hard-links.sh` also
scans `*.bzl`, `BUILD.bazel`, `MODULE.bazel` and `.bazelrc`. Because Bazel
and third-party rules write files Clax does not, `scripts/bazel-test.sh --ci`
ends by checking that the worktree's output base and the disk cache hold no
regular file with more than one link (`find -type f -links +1`). On
2026-10-15 changelist's output base, Bazel's shared download cache and its
install base on this machine held none, with rules_js and rules_rust in use.
Whether the rule covers files that Bazel's own rules create is Q8.

**No `pgrep -f`.** `bazel-gc.sh` and `bazel-test.sh` find processes by PID
files and the PIDs they start.

**Fast tests.** The tests themselves are unchanged: no sleeps, injected
clocks, perf in the perf gates.

## 14. CI

From phase 3, `ci.yml` runs `scripts/bazel-test.sh --ci` on Linux and gates
on it. It installs Bazelisk, Chromium's system libraries and, only for
`cargo metadata` in `bazel-sync.sh --check`, the Rust toolchain; Node, npm
packages and the compiler come from Bazel. It keeps `fetch-depth: 0` for the
plugin pin check. A failing run uploads `bazel-testlogs/**/test.log`,
`test.xml` and Playwright's results.

Persistence (Q3): with GitHub-hosted runners, Bazel's disk cache and
repository cache are an `actions/cache` entry keyed by `.bazelversion` and
the lockfiles, restored by prefix, and saved only by a push to main that
passed every step. A pull request reads main's cache and writes nothing.
`common:ci` sets `--lockfile_mode=error` and a 5-minute test timeout, as
changelist's does.

The native gate (`quality_gates.sh`) runs beside it on every pull request
until the parity check (§15, phase 3) has passed on 20 consecutive main
runs, then becomes a manual workflow (Q9).

`release.yml` does not change.

## 15. Migration

### 15.1 Enablers on main first

Small, native-neutral changes to shared files, each its own commit, landed
on main early so that the multi-person branch and others rebase over them
once rather than conflicting with a long-lived branch:

1. rust-embed folders through variables with today's paths as defaults
   (B10).
2. The `clax_embedded` cfg around the debug disk reads in
   `routes/shell.rs` (B11); a no-op under Cargo.
3. One test helper for the `clax` binary (`CLAX_TEST_BIN`, made absolute);
   `Command::cargo_bin("clax")` callers moved to it (B14).
4. Repository paths made absolute before reaching a child process (B12).
5. `CLAX_FAKE_EXE_DIR` in the three fake-executable helpers (§6.3).
6. `CLAX_TEST_NODE` in the real-Pi driver, once the multi-person branch has
   landed it (§5.6).

Each passes `scripts/quality_gates.sh` unchanged.

### 15.2 Phases

**Phase 1: Rust.** Everything in §5 and §6; rules_js for `//web:dist` and
`//plugins/pi:node_modules` only (B17); `scripts/run.sh`,
`build-system.sh`, `run.test.sh`, `bazel-sync.sh`, `bazel-test.sh --rust`,
`bazel-gc.sh` and their tests; `just test`, `just lint`, `just build`,
`just bazel-sync`, `just bazel-gc` and the `-native` twins; `docs/bazel.md`.
`just ci` stays `quality_gates.sh`. CI unchanged.
*Ships when:* every Rust test passes under Bazel on macOS and Linux; the
parity script (changelist's `bazel-parity.py`, adapted to nextest's list)
shows the same test names passing natively and under Bazel; §6.4, §9.2 and
§10.1's Rust rows are measured and hold; the output base has no hard links.
Q1 must be answered before it starts.

**Phase 2a: web, Pi and script gates.** §7.2 and §8's script and Pi rows;
`just web-test`, `just pi-test`, `just web`; `bazel-test.sh --ci` covers
every `quality_gates.sh` gate but browser tests and perf.
*Ships when:* those gates pass under Bazel, and a deleted module, test or
package is gone (`bazel-web.test.sh`).

**Phase 2b: browsers and perf.** Pinned Chromium, the e2e and time-to-usable
targets, the perf targets on the opt build; `just web-e2e`, `just perf`;
`just ci` and `just check` move to the overlay.
*Ships when:* `bazel-test.sh --ci` is a gate-for-gate match of
`quality_gates.sh` (a table in `docs/bazel.md`), the perf gates' quick runs
pass within budget under Bazel, and §10.1 holds for the whole suite.

**Phase 3: CI.** §14. *Ships when:* a Bazel CI run passes on main and its
cache restores on the next run.

### 15.3 Working beside the multi-person branch

- Bazel's files are additive: `MODULE.bazel`, `bazel/`, `BUILD.bazel` files,
  new scripts. The overlap with other branches is the justfile, the
  enablers (§15.1), and `Cargo.toml` changes that need a sync.
- BUILD files glob sources and tests, so new test modules (the multi-person
  branch adds many under `crates/clax-server/tests/`) need nothing; its new
  dependencies (`ring`) need only `just bazel-sync`.
- Until phase 3, nothing on main runs Bazel, so a branch that forgets the
  sync breaks no one's gate; the overlay names the command at the next Bazel
  run.
- The `bazel` branch rebases onto main after each large merge; phase 1
  lands after the multi-person branch if the two are ready together, since
  the canary tests are part of phase 1's parity.

## 16. Differences from Cargo

- **Features of third-party crates.** crate_universe keeps one feature list
  per crate and platform, so what a build script or proc macro asks of a
  crate the code beside it gets too; Cargo builds such a crate twice. The
  sync lists every such crate with its reason and fails on any other
  difference (§5.2).
- **Features of Clax's own crates in tests.** Tests link the variant with
  dev-dependency features (B9), as `cargo test -p` does. Under
  `cargo test --workspace` the features unify across the workspace, so the
  `clax` at `CARGO_BIN_EXE_clax` is built with `clax-server`'s test
  features; under Bazel the `clax` a test runs never has them (and neither
  does `quality_gates.sh`'s `CLAX_TEST_BIN`). Phase 1 finds any test that
  relied on the difference.
- **`CARGO_MANIFEST_DIR`** is the crate's path from the workspace root, not
  absolute; tests run from the runfiles root (B12).
- **`CARGO`** names nothing under Bazel (§5.3).
- **`CLAX_BUILD_COMMIT`** is `unknown` in every Bazel build (B13).
- **Embedded files in debug builds.** Bazel's debug `clax` carries the web
  UI, the extension and the plugins; Cargo's reads them from the checkout
  (B11).
- **Test processes.** Libtest in Bazel shards, not a process per test as
  under nextest; the same as `cargo test`.
- **Environment.** Actions and tests see a fixed `PATH` without Homebrew,
  their own `HOME` and temporary directory, and only declared files.
- **Build scripts** run in a sandbox with declared inputs; `rerun-if-changed`
  has no effect.
- **Debug information** follows Q7; Cargo's dev profile keeps line tables for
  the workspace's crates and none for dependencies.
- **Clippy** reads one `clippy.toml` per command; Clax has one, at the root.

## 17. Alternatives not taken

- **Building releases with Bazel.** Rejected for now (§11). Revisit if the
  release matrix moves to cross-compilation.
- **Changelist's `CARGO_BIN_EXE_` route** (compile outside the sandbox and
  the cache for an absolute path). Clax's tests take the binary at run time
  instead (B14), which keeps every test cached.
- **A runtime variable for the web UI's directory in debug builds**, instead
  of embedding (B11). Every spawned daemon would need it passed with an
  absolute path, and a debug binary would still read outside its sandbox.
- **rules_rust's rust-analyzer project generation.** The Cargo workspace
  already serves rust-analyzer, and the dev loop stays native.
- **Sharing changelist's disk cache.** Different compilers (1.94.0 and
  1.98.1) produce no common entries for Rust, so sharing only merges two
  budgets into one that either repository's trim can empty (Q2).

## 18. Open questions for the owner

**Q1. npm or pnpm for `web/` and `plugins/pi/`.** rules_js builds from
`pnpm-lock.yaml`.
(a) Switch both projects to pnpm, as changelist did: one lockfile per
project, nothing derived; `npm ci` becomes `pnpm install --frozen-lockfile`
in the native gates, and `bump-version.sh`, `test-release.sh` and `check-version.sh` follow
`plugins/pi`'s lockfile change. The published Pi package is unaffected.
(b) Keep npm and commit a `pnpm-lock.yaml` per project derived from
`package-lock.json`, checked by `bazel-sync --check`: two lockfiles to keep
in step.
Recommendation: (a). Needed before phase 1.

**Q2. Which disk cache.**
(a) Clax's own, `~/.cache/clax-bazel-disk`, 30 GB and 7 days.
(b) Changelist's `~/.cache/bazel-disk`, one bound for both (changelist's is
20 GB and already at 20 GB on this machine).
Recommendation: (a): the two repositories share no Rust outputs, and each
keeps a predictable budget.

**Q3. CI runner and cache for Bazel.**
(a) GitHub-hosted runners as today, with Bazel's disk cache in
`actions/cache` (10 GB per repository, downloaded and uploaded per run).
(b) Blacksmith runners with a sticky disk per job, as changelist.
Recommendation: (a) for phase 3, measured; (b) if a cached run's restore
takes more than a minute or the cache outgrows 10 GB.

**Q4. Where the `clax` that tests run lives.**
(a) In each worktree's output base, run once by the warm-up action: one
assessment per build per worktree, entirely inside Bazel.
(b) A machine-wide copy per content (`scripts/stable-bin.sh`'s store,
shared by all worktrees), named to the tests by a file the warm-up writes:
one assessment per content per machine, but tests run a file outside
Bazel's control, which can be pruned between build and test.
Recommendation: (a), and (b) only if §6.4's measurements show stalls.

**Q5. When Bazel saves disk but an edit runs slower than Cargo.**
(a) Speed wins: the task stays native by default until Bazel matches it.
(b) Disk wins: Bazel becomes the default within two minutes even if slower
than Cargo.
Recommendation: (a), per B20.

**Q6. Cached perf results.** A passed perf gate whose inputs (release
binary, scripts, budgets) are unchanged is not rerun.
(a) Cache them like any test.
(b) Never cache perf gates (`no-cache`): every `just ci` measures again.
Recommendation: (a): the gates judge the code, and unchanged code measured
on the same machine has passed; `--nocache_test_results` remeasures.

**Q7. Debug information in Bazel's test binaries.**
(a) Match Cargo's dev profile: line tables for the workspace's crates, none
for dependencies.
(b) Strip symbols, as changelist does (it measured 1.1 GB instead of 1.8 GB
per edited worktree's tests); a panic still names file and line, a
backtrace needs `-c dbg`.
Recommendation: (a), so a failing test's backtrace reads as under Cargo;
(b) if §9.2's edited-worktree budget is not met.

**Q8. Hard links made by Bazel's own rules.** Clax's files and scripts make
none (§13).
(a) The rule covers every file in Bazel's directories too: the check after
each `--ci` run fails on any, and a rule found making them is configured off
or replaced.
(b) It covers Clax's code, tests and scripts only.
Recommendation: (a): it costs one `find`, and none exist today.

**Q9. The native gate in CI after phase 3.**
(a) Run both on every pull request until 20 consecutive main runs agree,
then the native gate becomes a manual workflow, as changelist's.
(b) Keep both on every pull request for good.
Recommendation: (a).
