# Building with Bazel

Clax builds everything with Bazel when Bazel is installed: every `just`
recipe (`ci`, `check`, `test`, `web-test`, `pi-test`, `web-e2e`, `lint`,
`build`, `web`, `dev`, `watch`, `serve`, `install`, …), in every worktree,
and CI. The point is the whole experience in the common case: about five
worktrees at once, with a shared cache one of them has already filled. A
new worktree then needs no setup (no `pnpm install`, no web build, no
warm-up step) and no `target/` or `node_modules`: `just ci` and `just test`
work at once, from the cache, and the worktree costs next to no disk until
it edits ("Five worktrees", below, has the numbers).

Cargo, `Cargo.lock`, each `package.json` and each `pnpm-lock.yaml` stay the
source of truth; Bazel's view of them is derived and checked. The native
tools still work: on a machine without Bazel the recipes fall back to them
and say so, the `-native` twins (`just ci-native`, `just test-native`, …)
and plain `cargo`/`pnpm` commands run them on purpose, and the release
workflow uses them.

## Use

Install [Bazelisk](https://github.com/bazelbuild/bazelisk) (`brew install
bazelisk`), which reads `.bazelversion` and fetches that Bazel, and
`ibazel` (`brew install ibazel`) for `just watch`. That is all the setup.

The recipes go through `scripts/run.sh`, which picks the build system by
what is on `PATH`:

| On `PATH` | What runs |
|---|---|
| `bazelisk`, or `bazel` at the version `.bazelversion` names | Bazel, for every recipe: what CI runs |
| Neither (a `bazel` of another version is not used: it would fail far from the cause) | The native tools (`scripts/quality_gates.sh`, `cargo`, `pnpm`), saying so before the task and in its last line |
| Neither Bazel nor the native tools | Nothing: one message names both ways to install |

`CLAX_BUILD` overrides the choice: `bazel` uses Bazel or fails with the
install command; `native` never looks for Bazel (what the `-native` twins
set); `auto` is the default. `scripts/run.sh which` says what `auto` picks,
and `scripts/run.sh --dry-run <task>` prints what a task would run
(`scripts/run.test.sh` tests the choice). CI does not go through
`scripts/run.sh`: it calls `scripts/bazel-test.sh --ci`, so a runner
without Bazel fails and never falls back.

In a worktree, use the recipes: no `cargo build`, `cargo test` or
`pnpm install` except through a `-native` twin. Each of those makes a
`target/` or a `node_modules` in the worktree, gigabytes Bazel's shared
cache does not need.

### The dev loop

- `just watch` runs `ibazel run //crates/clax-cli:clax -- serve …`: Bazel
  rebuilds the debug `clax`, which embeds the web UI, and restarts it on any
  Rust or web change; reload the browser after. Measured: about 12 s after a
  web shell edit, 30 s after a clax-core edit, on a loaded machine. Without
  Bazel it is the native watch (cargo-watch and Vite).
- `just dev <harness>` runs the harness with Bazel's debug `clax`
  (`scripts/run.sh bin` builds it and prints its path).
- `just serve`, `just stop`, `just doctor` run that `clax`
  (`scripts/run.sh clax …`).
- `just install` installs Bazel's optimized `clax` (`//crates/clax-cli:clax_opt`)
  as `$CARGO_HOME/bin/clax`, by a rename, then stops the agents' daemon if
  it ran the old one and runs `clax init`. Its `--version` names no commit
  ("Differences from Cargo"). `just install-native` is Cargo's build.

### rust-analyzer

`just rust-project` writes `rust-project.json` (git-ignored) from Bazel with
rules_rust's `gen_rust_project`; rerun it after adding a crate or a
dependency. Point rust-analyzer at it, and its check at Bazel, so the editor
never runs `cargo check` into a `target/`; for VS Code, in
`.vscode/settings.json`:

```json
{
  "rust-analyzer.linkedProjects": ["rust-project.json"],
  "rust-analyzer.check.overrideCommand": [
    "bazelisk", "build", "--@rules_rust//rust/settings:error_format=json", "//crates/..."
  ]
}
```

| Instead of | Run |
|---|---|
| `scripts/quality_gates.sh` | `just ci` (`scripts/bazel-test.sh --ci`) |
| `cargo nextest run --workspace` | `just test`, or `scripts/bazel-test.sh --rust` |
| `cargo nextest run -p clax-server -E 'test(shell)'` | `bazelisk test //crates/clax-server:integration --test_arg=shell` |
| `cargo build -p clax-cli` | `bazelisk build //crates/clax-cli:clax` (`bazel-bin/crates/clax-cli/clax`) |
| `pnpm run lint`, `typecheck`, `test` in `web/` | `just web-test`, or `bazelisk test //web:lint //web:typecheck //web:unit` |
| `pnpm run e2e` in `web/` | `just web-e2e` (`//web:e2e_gesture`, `//web:e2e_chrome_overlay`, `//web:e2e_rest`) |
| `scripts/build-web.sh` | `just web` (copies of Bazel's build into `web/dist` and `web/dist-extension`) |

Use `scripts/bazel-test.sh` rather than `--config=clippy` by hand: it gives
`crates/clax-commands` its own `clippy.toml`, which Bazel takes only one of per
command. It holds off idle sleep on macOS while it runs; a plain
`bazelisk test` does not (`caffeinate -i bazelisk test …` does).

## Keeping Bazel in step with Cargo

Most changes need nothing: a new source file, a new module under `tests/`,
a new use of a crate already in a `Cargo.toml`. A change to any
`Cargo.toml` or `Cargo.lock` needs `just bazel-sync`, committed with it.
`scripts/bazel-sync.sh` writes:

- `bazel/workspace.bzl`: each crate's version, targets (`[[test]]`
  included), features, path dependencies and dev-dependencies, its build
  script, and the workspace lints as rustc flags (a dev-dependency that
  turns a workspace crate's features on is refused, not derived);
- `bazel/release_files.bzl`: `scripts/test-release.files` as labels;
- `bazel/playwright/browsers.bzl`: the Chromium the locked Playwright runs,
  with checksums (rewritten only when the locked version moves);
- `MODULE.bazel.lock`.

`scripts/bazel-sync.sh --check` (the first step of every Bazel run, and of
CI) writes nothing and fails naming `just bazel-sync` when any of these is
stale. It also fails when `MODULE.bazel`'s Rust version differs from
`rust-toolchain.toml`, a workspace crate has no `BUILD.bazel`, `.bazelrc`
limits jobs or cores, or a third-party crate's features under Bazel differ
from `cargo tree --workspace`'s beyond `NATIVE_EXTRA` in
`scripts/bazel-derive.py`. A merge conflict in a derived file is resolved
by running the sync on the merged manifests. `scripts/bazel-sync.test.sh`
tests the check.

## Targets

Each crate's `BUILD.bazel` calls `cargo_package()` (`bazel/cargo.bzl`),
which declares, with `<lib>` the library's crate name:

- `<lib>`; no crate has a test-only feature or variant: test support is
  the `clax-testkit` crate, and `scripts/bazel-derive.py` refuses a
  dev-dependency that turns features on;
- `<lib>_lib_test` (unit tests), each binary and its `<bin>_bin_test`,
  and each `[[test]]` under its name (`//crates/clax-cli:descriptors`);
- for the `integration` tests of clax-server, clax-cli and clax-mcp, the
  modules of `tests/integration.rs` dealt into a few targets (six, three
  and two: `//crates/clax-server:integration__1` … `__6`; `split_tests`).
  Each compiles the shared `common` module and its own modules, so the
  groups build and run in parallel and an edit to one test file rebuilds
  one group. More groups would repeat more of what every test binary
  pays (codegen of the dependencies' generics, a link): one target per
  module made a clax-core edit compile eight times as much. The root gates
  each module with `#[cfg(any(not(clax_split), clax_split = "<module>"))]`;
  Cargo sets no `clax_split` and builds them all into one binary, as before;
- `build_script` when the crate has one;
- `rust_sources`, for source scans, and `manifest`, the crate's
  `Cargo.toml`, for the layering check.

A BUILD file names only what a crate reads outside `src/`: what it embeds,
what its tests read. `//crates/clax-cli:clax_warm` runs the binary once
("macOS first runs"); `//crates/clax-cli:clax_opt` is the optimized build
(`bazel/opt.bzl`) the perf benchmarks run and `--ci` builds, as the native
checks build the release binary. `//web:dist` (with `dist_extension`,
`dist_extension_test`, `dist_e2e`) is the web build; `//web:lint`,
`//web:typecheck`, `//web:unit`, `//web:bundle_size`,
`//plugins/pi:typecheck` and `//plugins/pi:test` the web and Pi checks;
`//scripts:*` the script gates. The perf benchmarks
(`//scripts:perf_daemon_full`, `//scripts:perf_clients_full`,
`//web:perf_usable_full`) are `manual`: performance is measured on demand
(`just perf`, which reruns them and prints their numbers) and gates nothing.

A crate whose compile inputs include a generated directory has its
sources placed in `bazel-bin` by rules_rust, so the repository files its
sources include sit there too (the `*_bin` copies in the BUILD files).
`clax-embedded` is the one such crate: it alone embeds the web UI, the
extension and the plugins, so a web edit recompiles it and the binary, not
the daemon. Which crate may depend on which is
`scripts/check-crate-layers.py` (`//scripts:crate_layers`), from
`docs/superpowers/specs/2026-10-16-workspace-layout-design.md` §2.1.

## macOS first runs

macOS assesses every new executable on its first run, which can take
seconds on a loaded machine. Bazel makes a new file for `clax` in each
output base, so `warm_binary` (`bazel/warm.bzl`) runs it in an action that
is never taken from any cache (`no-cache`; `no-remote-cache` does not keep
an action out of the disk cache), and every test that runs `clax` depends
on that action's stamp. Once the file is assessed the action costs one
`clax --version`, and its unchanged stamp reruns nothing. Tests then run the same
file through runfiles links. The fake executables keep one store across
tests and runs (`CLAX_FAKE_EXE_DIR`, set by `scripts/bazel-test.sh`). Tests
find `clax` through `clax_fake_exe::clax_bin`, which makes `CLAX_TEST_BIN`
absolute.

## The web side and Pi

`web/` and `plugins/pi/` install with pnpm (`scripts/pnpm-install-stamped.sh`
natively). Under Bazel, rules_js fetches each project's packages from its
lockfile (`@npm_web`, `@npm_pi`) and runs the project's own `package.json`
scripts through `bazel/js/npm_script.mjs` (taken from `../changelist`): in
a private copy of exactly the declared files, so a deleted module or
package is really gone (`scripts/bazel-web.test.sh` checks this). Inputs
named in `skips` (the warmed `clax`) are read where Bazel put them rather
than copied. `scripts/pnpm-lock-check.py` fails when a `package.json` and
its lockfile disagree. Node is the toolchain `MODULE.bazel` pins (22, as
CI's native job uses); the real-Pi Rust tests get it as `CLAX_TEST_NODE`.

## Browser tests

The browser tests and time to usable run on a Chromium pinned by checksum
(`@chromium//:browsers`, `bazel/playwright/repo.bzl`): the build the locked
Playwright would download, laid out as `PLAYWRIGHT_BROWSERS_PATH` expects.
Nothing downloads while a test runs. `web/e2e/bazel-env.ts` makes the paths
Bazel hands the tests absolute. The targets are tagged `no-sandbox`: under
macOS's sandbox the `rest` project ran in 31 minutes instead of one, and
user-activation tests timed out. In CI,
`bazelisk run //web:playwright -- install-deps chromium` installs the
system libraries Chromium needs.

## Run outside Bazel

`scripts/bazel-test.sh --ci` runs these as they are, after the Bazel
passes, because each reads git, runs `just` or `cargo`, writes into the
checkout, or runs Bazel itself: `scripts/test-plugins.sh` (git tags),
`scripts/test-justfile.sh`, `scripts/test-dev.sh`,
`scripts/test-extension-signing.sh` (writes `web/dist-extension`),
`scripts/check-no-hard-links.sh` and `scripts/check-pipefail-pipes.sh` over
the tracked files (their own tests are Bazel tests),
`scripts/bazel-sync.test.sh` and `scripts/bazel-web.test.sh`. The slow
ones are skipped when they passed last time on the same files, as Bazel
caches a passed test: the plugin and dev-script checks are keyed on the
whole working tree and the `v*` tags, the two Bazel-setup tests on the
Cargo, Bazel and package manifests and Bazel's own files. A pass is
recorded under `~/.cache/clax-checks/` (`CLAX_CHECKS_CACHE`) by its key, so
a new worktree at contents some worktree checked skips them too; delete the
directory to rerun them. CI keeps no such record and runs every one. Tests that
need a real harness CLI, a model or credentials (`scripts/verify-harnesses.sh`
against real harnesses, the smoke scripts) stay native and manual, as
before.

## Gate for gate

What each gate of `scripts/quality_gates.sh` is under `scripts/bazel-test.sh --ci`.
Performance gates nothing in either (`just perf` measures on demand).

| `quality_gates.sh` gate | Under Bazel (`--ci`) |
|---|---|
| web pnpm install, pi pnpm install | `scripts/pnpm-lock-check.py web plugins/pi`; rules_js installs from each lockfile |
| web build | `//web:dist`, `//web:dist_extension` (inputs of the tests and the binaries that embed them) |
| web unit | `//web:unit` |
| web bundle size | `//web:bundle_size` |
| web lint | `//web:lint` |
| web typecheck | `//web:typecheck` |
| pi extension | `//plugins/pi:typecheck`, `//plugins/pi:test` |
| playwright browser, web e2e | `//web:e2e_gesture`, `//web:e2e_chrome_overlay`, `//web:e2e_rest` (Chromium from `@chromium//:browsers`) |
| build clax, build tests | the build of `//crates/...` in the one `bazel test` |
| cargo nextest (or cargo test) | every Rust test target, `tests(//crates/...)`, the integration tests in a few targets per crate |
| cargo fmt --check | `--config=fmt` over `//crates/...` |
| cargo clippy | `--config=clippy` over `//crates/...`: each library, binary and test target; `crates/clax-commands` again with its own `clippy.toml` |
| release build | `//crates/clax-cli:clax_opt` |
| justfile | `scripts/test-justfile.sh`, outside Bazel |
| release scripts | `//scripts:release` |
| tool hook gate | `//scripts:tool_hook` |
| extension signing | `scripts/test-extension-signing.sh`, outside Bazel |
| stable bin | `//scripts:stable_bin` |
| fake exe | `//scripts:fake_exe` |
| release installer | `//scripts:installer` |
| dev scripts | `scripts/test-dev.sh`, outside Bazel |
| plugins | `scripts/test-plugins.sh`, outside Bazel |
| plugin wrapper | `//scripts:plugin_wrapper` |
| comment loop | `//scripts:comment_loop` |
| worker npm ci, worker cargo fmt, worker cargo test, worker cargo clippy, worker wasm, worker e2e | CI's `worker` job, natively (`just worker-test`, then `e2e.mjs`): `crates/clax-worker` is a workspace of its own, outside Bazel |
| wasm32 check | CI's `worker` job, natively (`scripts/check-wasm.sh`, `just check-wasm`) |
| no unsafe code | `//scripts:no_unsafe` |
| crate layers | `//scripts:crate_layers` |
| no hard links | `//scripts:no_hard_links` (the check's own test), and `scripts/check-no-hard-links.sh` over the tracked files outside Bazel |
| pipefail pipes | `//scripts:pipefail_pipes`, and `scripts/check-pipefail-pipes.sh` over the tracked files outside Bazel |

Bazel adds: `scripts/bazel-sync.sh --check` first, `//scripts:bazel_gc`,
`//scripts:run_choice`, `//bazel/js:scratch_test`,
`scripts/bazel-sync.test.sh` and `scripts/bazel-web.test.sh`.

The hard-link rule covers the links Clax's own files make, not those of the
tools it runs. The owner's ruling of 2026-10-16: "Hard links that we create are banned, not those created by well tested tools". So `--ci` no longer
scans Bazel's output base or the disk cache for files with more than one
link; the source and rule scans (`scripts/check-no-hard-links.sh`) stay.

## CPU and memory

Bazel runs as many actions as there are cores. Rust tests run four libtest
threads each (`RUST_TEST_THREADS=4`); the browser projects take eight
workers each and declare it (`cpu:8`). The perf benchmarks are `exclusive`:
Bazel runs them one at a time with nothing beside them. To hold builds
back on a shared machine, copy `user.bazelrc.example` to `user.bazelrc`.

## CI

CI (`.github/workflows/ci.yml`) runs `scripts/bazel-test.sh --ci` on a
GitHub-hosted runner. `.github/actions/setup-bazel` installs Bazelisk and
restores `~/.cache/bazel-disk` and `~/.cache/bazel-repo` with
`actions/cache`, keyed by the lockfiles and the commit, falling back to the
newest cache of the same lockfiles and then of any; only a passing push to
main saves one, after `scripts/bazel-gc.sh` trims the disk cache to 6 GB
(Bazel trims only while its server sits idle, which a job never does). `common --config=ci` (`.bazelrc`) bounds the cache to what
fits a runner and fails on a stale lockfile. The native gates stay as a
manual workflow (`native-check.yml`). A second job, `worker`, runs what
Bazel does not build, natively: `scripts/check-wasm.sh` over the crates a
hosted Worker runs, and `crates/clax-worker` (a workspace of its own, for
wasm32 only): `just worker-test`, then its browser e2e (`e2e.mjs`) on a
web UI built with pnpm. A third job, `perf-log`, runs the
perf benchmarks after every green push to main and appends a record to the
`perf-log` branch (`docs/usage.md`, "Developing Clax"); it gates nothing, and is
the only job with write access.

## Disk

Bazel's files live outside the repository, in three places:

- **The shared compile cache**, `~/.cache/bazel-disk`: every action's
  outputs, for every worktree of this repository and of `../changelist`.
  Bazel trims it to 100 GB and to entries used in the last 7 days while a
  server sits idle; the 100 GB bound covers both repositories, whose
  `.bazelrc` files state the same bound. Change the two together.
- **Bazel's user root**, `~/Library/Caches/bazel/_bazel_$USER` on macOS:
  - one output base per worktree, of either repository (its
    `DO_NOT_BUILD_HERE` names the worktree), holding that worktree's
    outputs and its Bazel server. Files that came from the shared cache are
    APFS clones of its entries, so `du` counts them and the disk mostly
    does not;
  - `cache/repos/v1`, shared by every output base of every repository:
    `content_addressable` holds the downloads (toolchains, crates, npm
    packages, Chromium), `contents` the external repositories unpacked once
    for all output bases. Bazel removes an unpacked repository unused for
    a while on its own; nothing in this repository trims either;
  - `install`, Bazel's own binaries per version.
- **The checks' pass records**, `~/.cache/clax-checks`: empty files, one
  per passing keyed outside check ("Run outside Bazel").

`just bazel-gc` (`scripts/bazel-gc.sh`) applies the shared cache's bounds
now, removes the output bases of this repository's removed worktrees (a
server judged dead by `kill -0` on its PID file; changelist's output bases
are its own `bazel-gc`'s), and with `--report` prints real sizes. It warns
when `../changelist/.bazelrc` states another bound. Run it after removing
worktrees.

A worktree that only runs the recipes has no `target/` and no
`node_modules`: those come only from the native tools (a `-native` twin, a
plain `cargo` or `pnpm`, or a machine without Bazel), whose state is
per worktree and large (this branch's own worktree, used for native
comparisons, holds a 29 GB `target/`, 18 GB of it Cargo's incremental
state). `just clean` removes them.

## Speed

Measured 2026-10-16 on the owner's 12-core machine, shared with other
sessions' builds; each figure has the 1-minute load average beside it, and
any figure above a load of about 15 is noise more than signal.

| Case | Bazel | Native | Load |
|---|---|---|---|
| `just ci`, nothing changed | 12 s, 14 s, 20 s | 188 s, 192 s | 6–9, 38 / 10–49 |
| `just ci` in a fresh worktree at a commit another built | 34 s (155 s before the outside checks' passes were shared) | 644 s (cold `target/`, 6.7 GB) | 7–33 |
| `just test` after a clax-core comment edit | 96 s, 140 s | 86 s, 89 s | 21–35 |
| `just web-test` after a web shell edit | 43 s, 44 s | 26 s, 27 s | 10–17 |
| `just ci` after a clax-core edit | 433 s, 467 s | 306 s, 307 s | 7–14 |
| `just ci` after a web shell edit | 280 s, 293 s | 155 s, 262 s | 7–37 |
| `just watch`: restart after a web shell / clax-core edit | 12 s / 30 s | | 16–21 |

Bazel wins whenever the cache has the work: nothing changed, or a new
worktree. After an edit it is slower than Cargo. Where the time goes, after
a clax-core comment edit (Bazel's profile against `cargo test --no-run
--timings`, the same edit):

| Unit | Cargo (incremental) | Bazel |
|---|---|---|
| clax-core library | 6.8 s | 9.4 s (+ 6.2 s metadata) |
| clax-core unit tests | 8.0 s | 21.7 s |
| clax-mcp library | 3.2 s | 7.1 s (+ 5.2 s metadata) |
| clax-server library | 3.8 s | 31.6 s, and 31.7 s again for its test-feature variant |
| clax-server unit tests | 5.3 s | 33.4 s |
| clax-server integration tests | 6.0 s | 31.0 s |
| clax-cli binary / its unit tests | 3.9 s / 3.7 s | 12.1 s / 11.1 s |
| clax-cli, clax-mcp integration tests | 3.3 s, 3.6 s | 11.6 s, 14.1 s |
| whole build of the tests | 24 s | critical path 73 s of compiling, then 42 s running clax-server's integration tests |

The difference is rustc's incremental compilation, which Cargo's dev
profile uses and rules_rust does not: Bazel compiles every crate that
depends on the edited one whole. Turning it on (unsandboxed rustc with
`-Cincremental` in the output base) cut the compile path to about 25 s, but
it stays off: its directory grows with every build, and "Disk filling up
is a bigger issue right now than slightly slower incremental builds" (the
owner, 2026-10-16). Every source edit changes the crate's
metadata, so every dependent recompiles. What was done instead: the
integration tests run as a few targets per crate (`split_tests`), the
format check, clippy and the tests share one invocation, clax-cli's own
clippy pass runs after it (from 70 s on the critical path to 8 s), the web
UI's build no longer reads tests, and the slow checks outside Bazel are
skipped when their inputs passed before.

The tables above predate the crate layout
(`docs/superpowers/specs/2026-10-16-workspace-layout-design.md`), which
took the store out of clax-core, split clax-server into five crates, put
every embedded file in clax-embedded and test support in clax-testkit, so
no crate is compiled twice. `just test` after an edit, measured
2026-10-16/17 on the same machine (1-minute load before → after):

| Edit | Before the layout | After it, first run | After it, second run |
|---|---|---|---|
| clax-core comment (`ids.rs`) | 105 s (9→19) | 74 s (23→21) | 93 s (11→42) |
| route comment (`clax-server/src/routes/threads.rs`) | 138 s (19→24) | 56 s (19→20) | 79 s (42→35) |
| web shell comment (`web/shell/index.html`) | 96 s (15→17) | 50 s (20→21) | 74 s (35→34) |

The second run shared the machine with a load above 30, which the test
runs (half of every critical path) feel most. What a clax-core edit
compiles now: clax-core (3.3 s; its unit tests 6.3 s), clax-store (7.5 s;
its tests 15.3 s), clax-mcp, then clax-server's library once (15–18 s, no
test-feature variant) and the test binaries. A web edit rebuilds
`web/dist`, clax-embedded (0.5 s) and the binary (3–4 s) and reruns the
tests that serve it; it recompiles no daemon crate.

## For agents

- Run checks through the recipes (`just ci`, `just test`, …); report a
  native result as native.
- After any `Cargo.toml` or `Cargo.lock` change, run `just bazel-sync` and
  commit the result.
- After removing worktrees, run `just bazel-gc`.
- Never `bazelisk clean --expunge` to fix a cache problem in a worktree
  others share a server with; report it.

## Differences from Cargo

- **`CARGO_MANIFEST_DIR`** is the crate's path from the workspace root, and
  tests run from the runfiles root, which stands for the repository. Tests
  make a repository path absolute (`clax_fake_exe::repo()`) before handing
  it to a child.
- **`CARGO`** names nothing under Bazel; a test fallback that would run it
  fails loudly. Tests get `clax` through `CLAX_TEST_BIN`.
- **`CLAX_BUILD_COMMIT`** is `unknown` in every Bazel build.
- **Embedded files in debug builds.** Bazel's debug `clax` embeds the web
  UI, the extension and the plugins (`--cfg clax_embedded`, rust-embed's
  `debug-embed`); Cargo's reads them from the checkout. The one test of
  that, `web::tests::an_embedded_build_carries_the_bridge` in
  clax-embedded, runs only
  under Bazel (`scripts/bazel-parity.py` knows it).
- **Features.** crate_universe keeps one feature list per crate, so Bazel's
  third-party crates have the workspace's merged features, as
  `cargo test --workspace` does, plus `debug-embed` on rust-embed.
- **The test binary.** Bazel's tests run `clax` built with its own
  features only, as the native gates' `CLAX_TEST_BIN` is.
- **The testkit in unit tests.** Bazel builds a crate's unit tests
  (`<lib>_lib_test`) without `clax-testkit`, which is for integration
  tests only (`unit_dev_deps` in `bazel/cargo.bzl`); Cargo gives every test
  target all dev-dependencies, so `cargo test -p clax-server --lib` builds
  the testkit and the server under it first, and a unit test that names
  `clax_testkit` compiles under Cargo and fails to under Bazel.
- **Debug information.** Workspace crates keep line tables, as Cargo's
  dev profile does; dependencies have none.
- **PATH.** Tests see `/usr/bin:/bin:/usr/sbin:/sbin` (`.bazelrc`), not the
  developer's PATH.
