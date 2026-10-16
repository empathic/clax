# Building with Bazel

Clax builds and tests with its native tools (Cargo, pnpm, Vite) or with
Bazel. CI gates on Bazel, and the default `just` recipes run it when it is
installed. The native tools are unchanged: `just dev`, `just watch`,
rust-analyzer, `just install`, the release workflow and
`scripts/quality_gates.sh` use them, and every Bazel recipe has a `-native`
twin. Cargo, `Cargo.lock`, each `package.json` and each `pnpm-lock.yaml`
stay the source of truth; Bazel's view of them is derived and checked.

Bazel is for where builds repeat: agent worktrees and CI. Every worktree
shares one compile cache with every other (and with `../changelist`), so a
new worktree runs every check without a compiler run, a `pnpm install`, a
`target/` or a `node_modules`.

## Use

Install [Bazelisk](https://github.com/bazelbuild/bazelisk)
(`brew install bazelisk`). It reads `.bazelversion` and fetches that Bazel.

`just ci`, `just check`, `just test`, `just lint`, `just web-test`,
`just pi-test`, `just web-e2e`, `just perf`, `just build` and `just web` go
through `scripts/run.sh`, which picks the build system once, by what is
installed:

| On `PATH` | What runs |
|---|---|
| Bazelisk, or a `bazel` at the version `.bazelversion` names | Bazel: what CI runs |
| Neither (a `bazel` of another version is not used) | The native tools: `scripts/quality_gates.sh`, `cargo nextest`, `pnpm` |
| Neither Bazel nor the native tools | Nothing: one message names both ways to install |

A native run because Bazel was not found says so before the task and in
its last line; the exit status is the task's own. The last line of every
native run says the result came from the native tools: CI gates on Bazel,
and the two can differ ("Differences from Cargo").

`CLAX_BUILD` overrides the choice: `bazel` uses Bazel or fails with the
install command; `native` never looks for Bazel (the `-native` recipes:
`just ci-native`, `just test-native`, …); `auto` is the default.
`scripts/run.sh which` says what `auto` picks, and
`scripts/run.sh --dry-run <task>` prints what a task would run
(`scripts/run.test.sh` tests the choice). CI does not go through
`scripts/run.sh`: it calls `scripts/bazel-test.sh --ci`, so a runner
without Bazel fails and never falls back.

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
`crates/clax-cli` its own `clippy.toml`, which Bazel takes only one of per
command. It holds off idle sleep on macOS while it runs; a plain
`bazelisk test` does not (`caffeinate -i bazelisk test …` does).

## Keeping Bazel in step with Cargo

Most changes need nothing: a new source file, a new module under `tests/`,
a new use of a crate already in a `Cargo.toml`. A change to any
`Cargo.toml` or `Cargo.lock` needs `just bazel-sync`, committed with it.
`scripts/bazel-sync.sh` writes:

- `bazel/workspace.bzl`: each crate's version, targets (`[[test]]`
  included), features, path dependencies, the features its tests turn on in
  other workspace crates, its build script, and the workspace lints as
  rustc flags;
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

- `<lib>`, and `<lib>_test` for a library another crate's tests turn
  features on in (`clax_server_test`: `test-routes`, `test-support`);
- `<lib>_lib_test` (unit tests), each binary and its `<bin>_bin_test`,
  and each `[[test]]` under its name (`//crates/clax-cli:integration`);
- `build_script` when the crate has one;
- `rust_sources`, for source scans.

A BUILD file names only what a crate reads outside `src/`: what it embeds,
what its tests read. `//crates/clax-cli:clax_warm` runs the binary once
("macOS first runs"); `//crates/clax-cli:clax_opt` is the optimized build
the perf gates run (`bazel/opt.bzl`). `//web:dist` (with `dist_extension`,
`dist_extension_test`, `dist_e2e`) is the web build; `//web:lint`,
`//web:typecheck`, `//web:unit`, `//web:bundle_size`,
`//plugins/pi:typecheck` and `//plugins/pi:test` the web and Pi checks;
`//scripts:*` the script gates and the perf gates.

A crate whose compile inputs include a generated directory (the web UI)
has its sources placed in `bazel-bin` by rules_rust, so the repository
files its sources include sit there too (the `*_bin` copies in the BUILD
files).

## macOS first runs

macOS assesses every new executable on its first run, which can take
seconds on a loaded machine. Bazel makes a new file for `clax` in each
output base, so `warm_binary` (`bazel/warm.bzl`) runs it once in an action
that is never taken from the disk cache (`no-remote-cache`), and every test
that runs `clax` depends on that action's stamp. Tests then run the same
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
`scripts/bazel-sync.test.sh` and `scripts/bazel-web.test.sh`. Tests that
need a real harness CLI, a model or credentials (`scripts/verify-harnesses.sh`
against real harnesses, the smoke scripts) stay native and manual, as
before.

## CPU and memory

Bazel runs as many actions as there are cores. Rust tests run four libtest
threads each (`RUST_TEST_THREADS=4`); the browser projects take eight
workers each and declare it (`cpu:8`). The perf gates are `exclusive`:
Bazel runs them one at a time with nothing beside them. To hold builds
back on a shared machine, copy `user.bazelrc.example` to `user.bazelrc`.

## CI

CI (`.github/workflows/ci.yml`) runs `scripts/bazel-test.sh --ci` on a
GitHub-hosted runner. `.github/actions/setup-bazel` installs Bazelisk and
restores `~/.cache/bazel-disk` and `~/.cache/bazel-repo` with
`actions/cache`, keyed by the lockfiles and the commit, falling back to the
newest cache of the same lockfiles and then of any; only a passing push to
main saves one. `common --config=ci` (`.bazelrc`) bounds the cache to what
fits a runner and fails on a stale lockfile. The native gates stay as a
manual workflow (`native-check.yml`).

## Disk

Three things hold Bazel's files, all outside the repository:

- **The shared compile cache**, `~/.cache/bazel-disk`. Bazel trims it to
  80 GB and to entries used in the last 7 days while a server sits idle;
  the 80 GB bound covers this repository and `../changelist` together,
  whose `.bazelrc` states the same bound. Change the two together.
- **One output base per worktree**, under Bazel's user root
  (`bazelisk info output_base`). Its files are APFS clones of cache
  entries: `du` counts them, the disk does not.
- **Downloads and unpacked toolchains**, shared by every output base.

`just bazel-gc` (`scripts/bazel-gc.sh`) applies the cache's bounds now,
removes the output bases of removed worktrees (their server judged dead by
`kill -0` on its PID file), and with `--report` prints real sizes. It warns
when `../changelist/.bazelrc` states another bound. Run it after removing
worktrees.

## Speed

On a warm cache every Bazel recipe must be no slower than its native twin,
and the whole suite stays under two minutes.

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
  that, `an_embedded_debug_build_versions_the_embedded_bridge`, runs only
  under Bazel (`scripts/bazel-parity.py` knows it).
- **Features.** crate_universe keeps one feature list per crate, so Bazel's
  third-party crates have the workspace's merged features, as
  `cargo test --workspace` does, plus `debug-embed` on rust-embed.
- **The test binary.** Bazel's tests run `clax` built with its own
  features only, as the native gates' `CLAX_TEST_BIN` is.
- **Debug information.** Workspace crates keep line tables, as Cargo's
  dev profile does; dependencies have none.
- **PATH.** Tests see `/usr/bin:/bin:/usr/sbin:/sbin` (`.bazelrc`), not the
  developer's PATH.
