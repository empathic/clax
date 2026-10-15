# Bazel Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and test all of Clax with Bazel, as an optional overlay over Cargo and pnpm, with CI gating on Bazel, in one change.

**Architecture:** Changelist's setup (`../changelist`: `MODULE.bazel`, `.bazelrc`, `bazel/`, `scripts/run.sh`, `scripts/bazel-*.sh`) is the reference implementation; each task says what to copy and what Clax changes. Cargo, `Cargo.lock`, `package.json` and `pnpm-lock.yaml` stay the source of truth; Bazel's view of them is derived and checked. Native-neutral code changes come first, so the branch rebases over late multi-person fixes with conflicts confined to them.

**Tech Stack:** Bazel 9.3.0 via Bazelisk, rules_rust 0.74.0 (crate_universe), aspect_rules_js 3.5.1, rules_nodejs 6.7.6, bazel_lib 3.7.2, rules_shell 0.8.0, Rust 1.94.0, Node 22, pnpm, Playwright.

**Spec:** `docs/superpowers/specs/2026-10-15-bazel-overlay-design.md` (cited as "spec §N" and "spec Bn"). Read it before any task.

**Branch:** `bazel` in `/Users/alex/Devel/empathic/clax/.claude/worktrees/bazel`, on top of `multi-person`. One implementer at a time. Expect one more rebase onto `multi-person` (or main once it merges): after it, run `just bazel-sync` and Task 9's test pass before continuing.

## Global Constraints

- Bazel and rules are exactly: Bazel 9.3.0 (`.bazelversion`), rules_rust 0.74.0, aspect_rules_js 3.5.1, rules_nodejs 6.7.6, bazel_lib 3.7.2, rules_shell 0.8.0 (spec B4).
- Rust toolchain in `MODULE.bazel` equals `rust-toolchain.toml` (1.94.0), with a sha256 for every archive (spec B5).
- The disk cache is `~/.cache/bazel-disk`, shared with changelist: `--experimental_disk_cache_gc_max_size=40G`, `--experimental_disk_cache_gc_max_age=7d` until Task 17 sets the measured bound (spec §9.3).
- `CLAX_BUILD=bazel|native|auto` is the only switch; `auto` is the default (spec B2).
- No `unsafe` anywhere (`unsafe_code = "forbid"`); never `std::env::set_var` (it is `unsafe` in edition 2024).
- No hard links in code, tests, scripts, Starlark or `.bazelrc`; bazel_lib copy rules that take `hardlink` get `hardlink = "off"`.
- Never wait on processes by `pgrep -f`/`pkill -f`; use PIDs or PID files with a timeout.
- No sleeps standing in for a condition in tests; the whole check suite stays under two minutes on a warm cache, and every Bazel task is no slower than its native twin (spec B20, Q5).
- Tests never touch `~/.clax` or ports 7480/7481; scratch homes and port 0.
- Agents commit with `git -c commit.gpgsign=false commit`; never push.
- Commit messages and doc comments describe the change or the contract, never the conversation; prose writes "ID", not "id".
- Before editing a file, run `qualifier threads <path>`; record findings left for later as qualifier records, never TODO comments.
- Long runs go to a log file under the worktree (`target/` or `bazel-testlogs/`), run in the background, waited on in bounded stretches.

## Review Focus

1. **A second worktree at the same commit gets every output from the disk cache, including `clax`, but its file has never been run.** Expected: `clax_warm` reruns there (it is never served from the cache), so no test pays macOS's first-run assessment. Test: Task 8 Step 6 checks the warm action's tags with `bazel aquery`.
2. **A rebase brings a `Cargo.toml` or `Cargo.lock` change without `just bazel-sync`.** Expected: `scripts/bazel-test.sh` stops before building, naming `just bazel-sync`; it never rewrites derived files. Test: Task 6 Step 5.
3. **No Bazel installed (or a `bazel` of the wrong version).** Expected: `just ci` runs `scripts/quality_gates.sh`, says before and after that it ran natively and how to install Bazelisk; `CLAX_BUILD=bazel just ci` fails with the install command. Test: Task 14's `scripts/run.test.sh`.
4. **Changelist trims the shared cache to its own smaller bound.** Expected: Clax's `bazel-gc.sh` warns, naming both bounds, when `../changelist/.bazelrc` states a different one; it never deletes from a directory that is not a marked Bazel cache. Test: Task 15 Step 1.
5. **`just web` with Bazel, then `just dev`.** Bazel's outputs are read-only; copied as they are, Vite's next rebuild cannot overwrite `web/dist`. Expected: the copied files are writable regular files, never links. Test: Task 14 Step 1 (`just web` case in `run.test.sh`).

---

## File Structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/clax-fake-exe/src/lib.rs` (+ `src/bin.rs`) | test helpers: fake executables' store, the test `clax`, the repository root | 1, 2 |
| `scripts/fake-exe.sh`, `plugins/pi/test/fake-exe.ts`, `scripts/test-fake-exe.sh`, `plugins/pi/test/fake-exe.test.ts` | fake executables' store from `CLAX_FAKE_EXE_DIR` | 2 |
| `crates/clax-server/build.rs`, `crates/clax-cli/build.rs`, `routes/shell.rs`, `extension_files.rs`, includes | embeds and includes that work under both tools | 3 |
| `web/`, `plugins/pi/` package manager files, `scripts/pnpm-install-stamped.sh`, `scripts/pnpm-lock-check.py` | pnpm | 4 |
| `.bazelversion`, `.bazelrc`, `user.bazelrc.example`, `.bazelignore`, `BUILD.bazel`, `MODULE.bazel(.lock)`, `bazel/BUILD.bazel` | the Bazel workspace | 5 |
| `scripts/bazel-sync.sh`, `scripts/bazel-derive.py`, `bazel/workspace.bzl`, `scripts/bazel-sync.test.sh` | Cargo-derived files and their check | 6 |
| `bazel/js/*`, `bazel/dirs.bzl`, `web/BUILD.bazel`, `plugins/pi/BUILD.bazel` | rules_js and the web build | 7 |
| `bazel/cargo.bzl`, `crates/*/BUILD.bazel` | Rust targets | 8, 9 |
| `scripts/bazel-test.sh`, `scripts/bazel-parity.py` | the Bazel check runner | 9–13 |
| `scripts/BUILD.bazel`, `scripts/check-bazel-no-hard-links.sh` | script gates and scans as tests | 11 |
| `bazel/playwright/*` | pinned Chromium | 12 |
| `scripts/lib/build-system.sh`, `scripts/run.sh`, `scripts/run.test.sh`, `justfile`, `docs/bazel.md` | the overlay | 14 |
| `scripts/bazel-gc.sh`, `scripts/bazel-gc.test.sh` | disk | 15 |
| `.github/workflows/ci.yml`, `native-check.yml`, `.github/actions/setup-bazel/action.yml` | CI | 16 |

---

### Task 1: One helper for the test `clax` and the repository root

Every test that runs `clax` goes through one function, which takes `CLAX_TEST_BIN` made absolute, and every repository path handed to a child process is absolute (spec B12, B14). Native-neutral. Closes b42d4ebf (a bare `cargo nextest run` raced on `target/debug/clax` in `clax-mcp` and `clax-hooks`): the shared fallback keeps a copy by content.

**Files:**
- Create: `crates/clax-fake-exe/src/bin.rs`
- Modify: `crates/clax-fake-exe/src/lib.rs` (add `pub mod bin;` and re-exports)
- Modify: `crates/clax-mcp/tests/common.rs:6-41`, `crates/clax-hooks/tests/golden.rs:11-37`, `crates/clax-cli/tests/journal.rs:13-20`, `crates/clax-server/tests/golden_single_owner.rs:78-…`, `crates/clax-server/tests/common/clients.rs:31-82` (replace their `clax_bin`/`repo` with the shared ones)
- Modify: every `Command::cargo_bin("clax")` and `env!("CARGO_BIN_EXE_clax")` in `crates/clax-cli/tests/` (26 and 9 sites: `rg -n 'cargo_bin\(|CARGO_BIN_EXE_clax' crates`)
- Modify: test paths built from `env!("CARGO_MANIFEST_DIR")` that reach a child process or another directory: `crates/clax-cli/tests/{extension.rs:105,389, verify_harnesses.rs:18, grok_dedupe.rs:16, init.rs:10, version.rs:14}`, `crates/clax-hooks/tests/golden.rs:41` (use `clax_fake_exe::repo()`)
- Test: `crates/clax-fake-exe/src/bin.rs` (unit tests)

**Interfaces:**
- Produces:
  - `pub fn clax_fake_exe::repo() -> PathBuf` — the repository root, absolute, computed once (`OnceLock`).
  - `pub fn clax_fake_exe::clax_bin(cargo_bin_exe: Option<&'static str>) -> PathBuf` — absolute path of the `clax` to run: `CLAX_TEST_BIN` if set; else `cargo_bin_exe` (clax-cli's tests pass `option_env!("CARGO_BIN_EXE_clax")`) kept by content; else `cargo build -p clax-cli` kept by content. Computed once per process.
  - pure helpers `repo_from(manifest: &Path, cwd: &Path) -> PathBuf` and `absolute_from(path: PathBuf, cwd: &Path) -> PathBuf`, unit-tested.

- [ ] **Step 1: Write the failing unit tests**

`crates/clax-fake-exe/src/bin.rs`:

```rust
//! The `clax` binary the tests run, and the repository root.
//!
//! Under Cargo, `CARGO_MANIFEST_DIR` is absolute; under Bazel it is the
//! crate's path from the workspace root, and tests start in the runfiles
//! root, which then stands for the repository. Paths are made absolute once,
//! at first use, so a test that hands one to a child running elsewhere hands
//! it a path that still resolves.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The repository root for a crate whose manifest directory is `manifest`
/// (two levels below the root, `crates/<name>`), when the process started
/// in `cwd`.
pub fn repo_from(manifest: &Path, cwd: &Path) -> PathBuf {
    if manifest.is_absolute() {
        normalize(&manifest.join("../.."))
    } else {
        cwd.to_path_buf()
    }
}

/// `path`, absolute against `cwd` when it is relative.
pub fn absolute_from(path: PathBuf, cwd: &Path) -> PathBuf {
    if path.is_absolute() { path } else { normalize(&cwd.join(path)) }
}

/// `path` with `.` and `..` components removed lexically.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The repository root, absolute.
pub fn repo() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let cwd = std::env::current_dir().expect("a current directory");
        repo_from(Path::new(env!("CARGO_MANIFEST_DIR")), &cwd)
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_manifest_names_the_root_two_levels_up() {
        assert_eq!(
            repo_from(Path::new("/r/crates/clax-fake-exe"), Path::new("/elsewhere")),
            PathBuf::from("/r")
        );
    }

    #[test]
    fn a_relative_manifest_means_the_start_directory_is_the_root() {
        assert_eq!(
            repo_from(Path::new("crates/clax-fake-exe"), Path::new("/run/_main")),
            PathBuf::from("/run/_main")
        );
    }

    #[test]
    fn a_relative_binary_is_made_absolute_against_the_start_directory() {
        assert_eq!(
            absolute_from(PathBuf::from("crates/clax-cli/clax"), Path::new("/run/_main")),
            PathBuf::from("/run/_main/crates/clax-cli/clax")
        );
        assert_eq!(
            absolute_from(PathBuf::from("/abs/clax"), Path::new("/run")),
            PathBuf::from("/abs/clax")
        );
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p clax-fake-exe bin::`
Expected: FAIL to compile (`bin` module not declared).

- [ ] **Step 3: Add the module and `clax_bin`**

In `crates/clax-fake-exe/src/lib.rs` add `pub mod bin;` and `pub use bin::{clax_bin, repo};`. Append to `bin.rs`:

```rust
/// The `clax` the tests run, absolute: `CLAX_TEST_BIN` when set (the gates
/// and Bazel set it, to a binary run once already); else `cargo_bin_exe`
/// (Cargo's `CARGO_BIN_EXE_clax`, which only clax-cli's tests have); else a
/// `cargo build -p clax-cli`. A Cargo-built binary is kept by content
/// (`scripts/stable-bin.sh`) and run once there, so no test pays macOS's
/// first-run assessment and no later build replaces it under a run.
pub fn clax_bin(cargo_bin_exe: Option<&'static str>) -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let cwd = std::env::current_dir().expect("a current directory");
        if let Some(bin) = std::env::var_os("CLAX_TEST_BIN") {
            let bin = absolute_from(PathBuf::from(bin), &cwd);
            assert!(bin.exists(), "CLAX_TEST_BIN {} missing", bin.display());
            return bin;
        }
        let root = repo();
        let built = match cargo_bin_exe {
            Some(p) => PathBuf::from(p),
            None => {
                let mut cmd = std::process::Command::new(env!("CARGO"));
                cmd.current_dir(&root)
                    .args(["build", "--quiet", "-p", "clax-cli", "--bin", "clax"]);
                if !cfg!(debug_assertions) {
                    cmd.arg("--release");
                }
                assert!(cmd.status().expect("run cargo build").success(), "cargo build -p clax-cli failed");
                let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
                let target = std::env::var_os("CARGO_TARGET_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| root.join("target"));
                absolute_from(target, &root).join(profile).join("clax")
            }
        };
        let store = built.parent().unwrap().parent().unwrap().join("clax-bin/test");
        let out = std::process::Command::new(root.join("scripts/stable-bin.sh"))
            .arg(&built)
            .arg(&store)
            .output()
            .expect("run scripts/stable-bin.sh");
        assert!(out.status.success(), "stable-bin.sh: {}", String::from_utf8_lossy(&out.stderr));
        absolute_from(PathBuf::from(String::from_utf8(out.stdout).unwrap().trim()), &root)
    })
    .clone()
}
```

- [ ] **Step 4: Run the unit tests**

Run: `cargo test -p clax-fake-exe`
Expected: PASS.

- [ ] **Step 5: Move every caller to the helper**

- Delete the five private `clax_bin` functions and `clients.rs`'s `repo()`; call `clax_fake_exe::clax_bin(None)` and `clax_fake_exe::repo()` (in `crates/clax-cli/tests`, `clax_fake_exe::clax_bin(option_env!("CARGO_BIN_EXE_clax"))`, wrapped once in `crates/clax-cli/tests/common/mod.rs` as `pub fn clax() -> std::path::PathBuf`).
- Replace `Command::cargo_bin("clax").unwrap()` with `assert_cmd::Command::new(common::clax())`, and `std::process::Command::cargo_bin("clax")` with `std::process::Command::new(common::clax())`; replace `env!("CARGO_BIN_EXE_clax")` with `common::clax()` (as `&Path` or `.to_str().unwrap()` where a `&str` was used; `bin.rs:49`'s `const ME` becomes a function).
- Replace the listed `CARGO_MANIFEST_DIR`-based repository roots with `clax_fake_exe::repo()`.

Check nothing is left: `rg -n 'cargo_bin\(|CARGO_BIN_EXE_clax|fn clax_bin' crates` prints only `crates/clax-fake-exe/src/bin.rs` and the one `option_env!` in `crates/clax-cli/tests/common/mod.rs`.

- [ ] **Step 6: Run the Rust tests both ways**

Run: `cargo build -q -p clax-cli && CLAX_TEST_BIN="$PWD/$(scripts/stable-bin.sh target/debug/clax target/clax-bin/debug)" cargo nextest run --workspace > target/task1.log 2>&1; tail -5 target/task1.log`
Expected: all pass.
Run: `env -u CLAX_TEST_BIN cargo test -p clax-mcp --test integration open_status > target/task1b.log 2>&1; tail -3 target/task1b.log`
Expected: PASS (the fallback builds and keeps a copy).

- [ ] **Step 7: Commit, and close b42d4ebf**

```bash
git add crates
git -c commit.gpgsign=false commit -m "Run the tests' clax through one helper that makes CLAX_TEST_BIN and repository paths absolute"
```

Then, in a separate commit, resolve b42d4ebf with the evidence (the command from Step 6 and its output) and `--ref git:<sha>`, as `AGENTS.md` describes.

---

### Task 2: A fake-executable store that outlives one test

`CLAX_FAKE_EXE_DIR` names the store for the three fake-executable helpers (spec §6.3). Native-neutral: unset, the store is where it is today.

**Files:**
- Modify: `crates/clax-fake-exe/src/lib.rs:113` (`shared_copy`)
- Modify: `scripts/fake-exe.sh:33-34`
- Modify: `plugins/pi/test/fake-exe.ts:36`
- Create: `scripts/test-fake-exe.sh`, `plugins/pi/test/fake-exe.test.ts`
- Modify: `scripts/quality_gates.sh` (`lane_scripts` runs `scripts/test-fake-exe.sh`)

**Interfaces:**
- Produces: `pub fn clax_fake_exe::store_dir_from(var: Option<std::ffi::OsString>, tmp: &Path) -> PathBuf`; environment variable `CLAX_FAKE_EXE_DIR` (absolute path) honoured by all three helpers.

- [ ] **Step 1: Write the failing tests**

Rust, in `lib.rs`'s tests:

```rust
#[test]
fn the_store_is_clax_fake_exe_dir_when_set() {
    assert_eq!(
        store_dir_from(Some("/s/fakes".into()), Path::new("/tmp")),
        PathBuf::from("/s/fakes")
    );
    assert_eq!(store_dir_from(None, Path::new("/tmp")), PathBuf::from("/tmp/clax-fake-exe"));
    assert_eq!(store_dir_from(Some("".into()), Path::new("/tmp")), PathBuf::from("/tmp/clax-fake-exe"));
}
```

`scripts/test-fake-exe.sh`:

```bash
#!/usr/bin/env bash
# Checks that scripts/fake-exe.sh keeps its store in CLAX_FAKE_EXE_DIR when
# set, and in $TMPDIR/clax-fake-exe when not.
set -euo pipefail
cd "$(dirname "$0")/.."
d="$(cd "$(mktemp -d)" && pwd -P)"
trap 'chmod -R u+w "$d" 2>/dev/null; rm -rf "$d"' EXIT
(
    export CLAX_FAKE_EXE_DIR="$d/store"
    # shellcheck source=scripts/fake-exe.sh
    . scripts/fake-exe.sh
    printf '#!/bin/sh\necho hi\n' | fake_exe "$d/bin/hi"
    [ "$("$d/bin/hi")" = hi ]
    target="$(readlink "$d/bin/hi")"
    case "$target" in "$d/store/"*) ;; *) echo "FAIL: linked to $target, not under $d/store" >&2; exit 1 ;; esac
)
(
    export TMPDIR="$d/tmp"; mkdir -p "$TMPDIR"; unset CLAX_FAKE_EXE_DIR
    . scripts/fake-exe.sh
    printf '#!/bin/sh\necho hi\n' | fake_exe "$d/bin2/hi"
    case "$(readlink "$d/bin2/hi")" in "$d/tmp/clax-fake-exe/"*) ;; *) echo "FAIL: default store" >&2; exit 1 ;; esac
)
echo "PASS: fake-exe.sh store"
```

`plugins/pi/test/fake-exe.test.ts`:

```ts
import { mkdtempSync, readlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, test } from "vitest";
import { fakeExe } from "./fake-exe";

const saved = process.env.CLAX_FAKE_EXE_DIR;
afterEach(() => { if (saved === undefined) delete process.env.CLAX_FAKE_EXE_DIR; else process.env.CLAX_FAKE_EXE_DIR = saved; });

test("the store is CLAX_FAKE_EXE_DIR when set", () => {
  const d = mkdtempSync(join(tmpdir(), "clax-fake-test-"));
  process.env.CLAX_FAKE_EXE_DIR = join(d, "store");
  const at = fakeExe(join(d, "bin", "hi"), "#!/bin/sh\necho hi\n");
  expect(readlinkSync(at).startsWith(join(d, "store") + "/")).toBe(true);
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p clax-fake-exe the_store; bash scripts/test-fake-exe.sh; (cd plugins/pi && npx vitest run test/fake-exe.test.ts)`
Expected: Rust fails to compile; the script prints a FAIL line; vitest fails.

- [ ] **Step 3: Implement**

Rust: add

```rust
/// The store for shared copies: `CLAX_FAKE_EXE_DIR` when set and not empty
/// (Bazel's tests set it, so the store outlives one test's temporary
/// directory), else `<tmp>/clax-fake-exe`.
pub fn store_dir_from(var: Option<std::ffi::OsString>, tmp: &Path) -> PathBuf {
    match var {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => tmp.join("clax-fake-exe"),
    }
}
```

and in `shared_copy` replace `std::env::temp_dir().join("clax-fake-exe")` with `store_dir_from(std::env::var_os("CLAX_FAKE_EXE_DIR"), &std::env::temp_dir())`.

`scripts/fake-exe.sh:33-34` becomes:

```bash
FAKE_EXE_DIR="${TMPDIR:-/tmp}"
FAKE_EXE_DIR="${CLAX_FAKE_EXE_DIR:-${FAKE_EXE_DIR%/}/clax-fake-exe}"
```

`plugins/pi/test/fake-exe.ts:36` becomes `const dir = process.env.CLAX_FAKE_EXE_DIR || join(tmpdir(), "clax-fake-exe");`. Update each helper's header comment with one sentence naming `CLAX_FAKE_EXE_DIR`.

Add `run "fake exe"            scripts/test-fake-exe.sh &&` to `lane_scripts` in `scripts/quality_gates.sh`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p clax-fake-exe && bash scripts/test-fake-exe.sh && (cd plugins/pi && npx vitest run test/fake-exe.test.ts)`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/clax-fake-exe scripts/fake-exe.sh scripts/test-fake-exe.sh scripts/quality_gates.sh plugins/pi/test/fake-exe.ts plugins/pi/test/fake-exe.test.ts
git -c commit.gpgsign=false commit -m "Keep fake executables in CLAX_FAKE_EXE_DIR when it is set"
```

---

### Task 3: Embeds and includes that work under Cargo and Bazel

Generated directories reach rust-embed through variables with today's paths as defaults; compile-time includes are file-relative; a `clax_embedded` cfg makes a debug build that embeds its web UI read it from the embed (spec B10, B11). Native-neutral.

**Files:**
- Create: `crates/clax-server/build.rs`
- Modify: `crates/clax-cli/build.rs` (emit `CLAX_EXTENSION_DIST_DIR`)
- Modify: `Cargo.toml` (`rust-embed` gains `interpolate-folder-path`; `[workspace.lints.rust]` gains `unexpected_cfgs = { level = "warn", check-cfg = ['cfg(clax_embedded)'] }`)
- Modify: `crates/clax-server/src/routes/shell.rs:13-14, 85-110` (folder variable; debug `bridge_version` under `clax_embedded`)
- Modify: `crates/clax-cli/src/extension_files.rs:10` (folder variable)
- Modify: `crates/clax-server/src/boot.rs:301-323`, `crates/clax-server/src/shell_route.rs:90-92`, `crates/clax-cli/src/commands/haiku.rs:58-60`, `crates/clax-server/tests/shell_boot.rs:11-13` (file-relative `include_str!`)

**Interfaces:**
- Produces: rustc environment variables `CLAX_WEB_DIST_DIR` (clax-server) and `CLAX_EXTENSION_DIST_DIR` (clax-cli), each a directory path relative to the crate's manifest directory or absolute. Each build script passes through an incoming value of the same name, else emits the source path (`<manifest>/../../web/dist/`, `<manifest>/../../web/dist-extension/`). Task 8 sets them through `build_script_env`.
- Produces: cfg `clax_embedded`. Under it a debug build with no `set_web_dist` override serves the embedded web UI and computes `bridge_version` once from it.

- [ ] **Step 1: Write the failing test**

In `crates/clax-server/src/routes/shell.rs`'s tests module:

```rust
/// A debug build with its web UI embedded (`clax_embedded`, Bazel's) names
/// the bridge by the embedded bundle, with no file on disk to read.
#[cfg(all(debug_assertions, clax_embedded))]
#[test]
fn an_embedded_debug_build_versions_the_embedded_bridge() {
    let v = bridge_version();
    assert_eq!(v.len(), 12, "{v:?}");
    assert_eq!(v, short_hash(&Assets::get(BRIDGE).unwrap().metadata.sha256_hash()));
}
```

- [ ] **Step 2: Run it to see it fail**

Build the test against a scratch copy of the web UI, remove the copy, then run the test, so no file on disk can answer for the embed. Save this as `target/task3.sh`; Step 4 runs it again:

```bash
set -euo pipefail
scripts/build-web.sh >/dev/null
w="$(mktemp -d)"; cp -R web/dist "$w/dist"
bin="$(CLAX_WEB_DIST_DIR="$w/dist/" RUSTFLAGS="--cfg clax_embedded" \
  cargo test -p clax-server --features rust-embed/debug-embed --lib --no-run --message-format=json 2>/dev/null \
  | python3 -c 'import json,sys
for m in map(json.loads, sys.stdin):
    if m.get("executable") and m.get("target", {}).get("kind") == ["lib"]: print(m["executable"])')"
rm -rf "$w"
"$bin" an_embedded_debug_build
```

Expected: FAIL. Before Step 3 nothing reads `CLAX_WEB_DIST_DIR`; the debug `bridge_version` stats `web/dist` on disk, so the test can only pass by accident. Run it with `web/dist/_clax/bridge.js` moved aside (and restore it after) to see the empty version.

- [ ] **Step 3: Implement**

`crates/clax-server/build.rs`:

```rust
//! Names the web UI's directory for the rust-embed of `routes/shell.rs`:
//! `CLAX_WEB_DIST_DIR` from the environment when set (Bazel sets it to its
//! build of the web UI), else the checkout's `web/dist`.

fn main() {
    println!("cargo::rerun-if-env-changed=CLAX_WEB_DIST_DIR");
    let dir = std::env::var("CLAX_WEB_DIST_DIR").unwrap_or_else(|_| {
        format!("{}/../../web/dist/", env!("CARGO_MANIFEST_DIR"))
    });
    println!("cargo::rustc-env=CLAX_WEB_DIST_DIR={dir}");
}
```

`crates/clax-cli/build.rs` `main()` gains the same for `CLAX_EXTENSION_DIST_DIR` (default `<manifest>/../../web/dist-extension/`), with a doc line in its header.

`shell.rs`: `#[folder = "$CLAX_WEB_DIST_DIR"]`. `extension_files.rs`: `#[folder = "$CLAX_EXTENSION_DIST_DIR"]`. `Cargo.toml`: `rust-embed = { version = "8", features = ["include-exclude", "interpolate-folder-path"] }`.

The debug `bridge_version` in `shell.rs` becomes:

```rust
/// The bridge version: a short hash of the bridge bundle, which bridge tags
/// carry as `/_clax/bridge.js?v=<version>`. Empty when the UI is not built.
///
/// A debug build reads `web/dist` from disk on every request, and `just dev`
/// rebuilds the bridge under a running daemon, so the version follows the
/// file: it is rehashed whenever the file's path, modification time or size
/// changes. A debug build that embeds its web UI (`clax_embedded`) and has
/// no `set_web_dist` override versions the embedded bundle, once.
#[cfg(debug_assertions)]
pub fn bridge_version() -> String {
    #[cfg(clax_embedded)]
    if WEB_DIST.read().unwrap_or_else(|e| e.into_inner()).is_none() {
        static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        return VERSION.get_or_init(hash_bridge).clone();
    }
    // … the existing stamp-based body, unchanged …
}
```

`asset()` needs no change: with no override it calls `Assets::get`, which `debug-embed` serves from the binary. `web_dist()`'s default becomes `env!("CLAX_WEB_DIST_DIR").into()` (the directory the build embedded), replacing `concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/dist")`.

Includes: replace each `include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/…"))` with the path relative to the including file, for example in `crates/clax-server/src/boot.rs` `include_str!("../../../web/shell/src/frame-src-cases.json")` and in `crates/clax-server/tests/shell_boot.rs` `include_str!("../../../web/…")` (count the `..` from each file's directory; the build fails if one is wrong).

- [ ] **Step 4: Run the tests both ways**

Run: `bash target/task3.sh`
Expected: PASS.
Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run -p clax-server -p clax-cli > target/task3.log 2>&1; tail -3 target/task3.log`
Expected: PASS, no `unexpected_cfgs` warning.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates
git -c commit.gpgsign=false commit -m "Name embedded build directories by variable, include repository files by relative path, and serve the embedded web UI from a debug build that embeds it"
```

---

### Task 4: pnpm for `web/` and `plugins/pi/`

Implements the decision on thread d95eeace (spec §7.1, Q1). Native checks keep working.

**Files:**
- Create: `web/pnpm-lock.yaml`, `plugins/pi/pnpm-lock.yaml` (`pnpm import`, then `pnpm install`), `web/pnpm-workspace.yaml`, `plugins/pi/pnpm-workspace.yaml` (`allowBuilds: {}`), `scripts/pnpm-install-stamped.sh`, `scripts/pnpm-lock-check.py` (copy `../changelist/scripts/pnpm-lock-check.py`)
- Delete: `web/package-lock.json`, `plugins/pi/package-lock.json`, `scripts/npm-ci-stamped.sh`
- Modify: `web/package.json` and `plugins/pi/package.json` (`"packageManager": "pnpm@<version>"`, the version `pnpm --version` prints; `web` gains `"build:e2e"`, below)
- Modify: `scripts/quality_gates.sh` (`pnpm-install-stamped.sh`, `pnpm exec playwright install …`, `pnpm run …`), `justfile` (`web`, `web-test`, `web-e2e`, `pi-test`, `perf`, `clean`), `scripts/watch.sh:60`, `scripts/demo-room-sample.sh:40`, `scripts/smoke-capabilities.sh:263-268`, `scripts/smoke-pi.sh:14,43`, `web/e2e/global-setup.ts`, `scripts/bump-version.sh:31-32`, `scripts/check-version.sh:24-34`, `scripts/test-release.sh:23`, `crates/clax-cli/tests/init.rs:150`, `.github/workflows/ci.yml:19`, `.github/workflows/release.yml:72`, `docs/usage.md`, `AGENTS.md`, `plugins/pi/README.md` where they say `npm ci`

**Interfaces:**
- Produces: `scripts/pnpm-install-stamped.sh <dir> [pnpm install args…]` (same contract as the deleted npm script: stamp `node_modules/.clax-install-stamp`, hash of `pnpm-lock.yaml`, `node --version`, `pnpm --version`; `CLAX_PNPM_INSTALL=always` ignores it); `web` script `build:e2e` writing `dist-e2e/` (the test-clock build); `web/e2e/global-setup.ts` uses a preset `CLAX_E2E_WEB_DIST` as is.

- [ ] **Step 1: Write the failing check**

`scripts/pnpm-lock-check.py web plugins/pi` (copied from changelist) is the check that each `package.json` and its lockfile agree. Run it now.
Expected: FAIL (no `pnpm-lock.yaml`).

- [ ] **Step 2: Convert the lockfiles**

```bash
for d in web plugins/pi; do (cd "$d" && pnpm import && rm package-lock.json && printf 'allowBuilds: {}\n' > pnpm-workspace.yaml && pnpm install --frozen-lockfile); done
scripts/pnpm-lock-check.py web plugins/pi
```

Expected: the check passes. Compare resolved versions with the old lockfiles (`git show HEAD:web/package-lock.json`) for `vite`, `svelte`, `vitest`, `@playwright/test`, `oxlint`, `@mariozechner/pi-coding-agent`: same versions.

- [ ] **Step 3: Replace npm in scripts, recipes and workflows**

`scripts/pnpm-install-stamped.sh`:

```bash
#!/usr/bin/env bash
# Runs `pnpm install --frozen-lockfile` in the given directory unless its
# node_modules was installed from the same pnpm-lock.yaml by the same Node
# and pnpm. The stamp, node_modules/.clax-install-stamp, holds a hash of all
# three and is written only after the install succeeds, so an interrupted
# install runs again next time.
#
# Usage: scripts/pnpm-install-stamped.sh <dir> [pnpm install args...]
# CLAX_PNPM_INSTALL=always ignores the stamp.
set -euo pipefail
dir="$1"; shift
cd "$dir"
stamp="node_modules/.clax-install-stamp"
want="$( { cat pnpm-lock.yaml; node --version; pnpm --version; } | shasum -a 256 | cut -d' ' -f1)"
if [ "${CLAX_PNPM_INSTALL:-}" != "always" ] && [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$want" ]; then
    echo "pnpm install: $dir/node_modules matches pnpm-lock.yaml; skipped"
    exit 0
fi
pnpm install --frozen-lockfile "$@"
printf '%s\n' "$want" > "$stamp"
```

In `web/package.json` add `"build:e2e": "node scripts/build-e2e.mjs"`. Move the test-clock build out of `web/e2e/global-setup.ts` into `web/scripts/build-e2e.mjs`, which builds into `dist-e2e/` from `dist/` exactly as `global-setup.ts` does today, with `vite` run through the `vite` package's JavaScript API instead of `npx`. `global-setup.ts` then: if `process.env.CLAX_E2E_WEB_DIST` is set, use it; else run its inputs-hash check and `pnpm run build:e2e` into the cache directory as today. Its inputs list names `pnpm-lock.yaml`.

`bump-version.sh` and `check-version.sh`: the Pi version lives only in `plugins/pi/package.json` (a pnpm lockfile carries no package version); drop the two `package-lock.json` entries and their tests in `test-release.sh`. CI workflows: add `pnpm/action-setup` (pinned by commit SHA, the version from `packageManager`) before `actions/setup-node` and set `cache: pnpm` with `cache-dependency-path: web/pnpm-lock.yaml`.

Check: `git grep -n -E '\bnpm (ci|run|test)\b|npx |package-lock' -- ':!*.md' ':!*.qual' ':!docs/superpowers'` prints only `scripts/smoke-pi.sh`'s `npx --yes @mariozechner/pi-coding-agent@…` fallback, which becomes `pnpm dlx @mariozechner/pi-coding-agent@0.73.1`, so nothing.

- [ ] **Step 4: Run the native gates**

Run: `scripts/quality_gates.sh > target/task4.log 2>&1; tail -40 target/task4.log`
Expected: `all gates passed`.

- [ ] **Step 5: Commit**

```bash
git add -A web plugins/pi scripts justfile .github docs/usage.md AGENTS.md crates/clax-cli/tests/init.rs
git status --short   # the two package-lock.json files and npm-ci-stamped.sh show as deleted; nothing under node_modules
git -c commit.gpgsign=false commit -m "Install the web UI's and the Pi extension's packages with pnpm"
```

---

### Task 5: The Bazel workspace, toolchains and repositories

**Files:**
- Create: `.bazelversion` (`9.3.0`), `.bazelrc`, `user.bazelrc.example`, `.bazelignore`, `BUILD.bazel`, `MODULE.bazel`, `bazel/BUILD.bazel`
- Create: `web/BUILD.bazel` and `plugins/pi/BUILD.bazel`, each for now only `exports_files(["pnpm-lock.yaml", "package.json", "pnpm-workspace.yaml"])`, so `MODULE.bazel` can name the lockfiles (Task 7 fills them)
- Modify: `.gitignore` (`/bazel-*`, `/user.bazelrc`)

**Interfaces:**
- Produces: repositories `@crates` (the workspace's crates from `Cargo.lock`), `@npm_web`, `@npm_pi`, `@rust_toolchains`, the Node toolchain; `//bazel:fastbuild` config setting; `.bazelrc` configs `clippy`, `fmt`, `ci`.

- [ ] **Step 1: Write the failing check**

Run: `bazelisk query @crates//:all`
Expected: FAIL (no workspace).

- [ ] **Step 2: Write the workspace files**

`MODULE.bazel`: start from `../changelist/MODULE.bazel`. Keep `rules_rust`, `rust.toolchain` (edition `2024`, `versions = ["1.94.0"]`, `rustfmt_version = "1.94.0"`, sha256s for `cargo`, `clippy`, `llvm-tools`, `rust-std`, `rustc`, `rustfmt` 1.94.0 on `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`; take each from `https://static.rust-lang.org/dist/<name>.tar.xz.sha256`), `crate.from_cargo(name = "crates", cargo_lockfile = "//:Cargo.lock", manifests = ["//:Cargo.toml"] + one per crate, supported_platform_triples` as changelist plus `x86_64-apple-darwin`). Drop everything about Wasm, the release server and sqlx. Add:

```starlark
# A Bazel debug build of clax embeds its web UI, extension and plugins
# (docs/superpowers/specs/2026-10-15-bazel-overlay-design.md B11).
crate.annotation(
    crate = "rust-embed",
    crate_features = ["debug-embed"],
    repositories = ["crates"],
)
```

Then rules_js as changelist, with `node.toolchain(node_version = "<the exact 22.x release CI's setup-node resolves>")` and two `npm.npm_translate_lock` calls: `name = "npm_web", pnpm_lock = "//web:pnpm-lock.yaml"` and `name = "npm_pi", pnpm_lock = "//plugins/pi:pnpm-lock.yaml"`; `bazel_lib` and `rules_shell` as changelist.

`.bazelrc`: copy changelist's, then:

```
# The compile cache, shared with ../changelist and every other project on
# this machine: one directory, one bound. ../changelist/.bazelrc states the
# same three lines; change both together (docs/bazel.md, "Disk").
build --disk_cache=~/.cache/bazel-disk
build --experimental_disk_cache_gc_max_size=40G
build --experimental_disk_cache_gc_max_age=7d
```

Remove changelist's `wasm`, `postgres` configs; keep `clippy`, `fmt`, `ci`; add `build --@rules_rust//rust/settings:pipelined_compilation=True` and `build --@rules_rust//rust/settings:clippy.toml=//:clippy.toml`. No `--jobs` or `--local_resources` line.

`.bazelignore`:

```
.claude
.superpowers
target
dist
node_modules
web/node_modules
web/dist
web/dist-extension
web/dist-extension-test
web/dist-e2e
plugins/pi/node_modules
```

`BUILD.bazel`: `exports_files(["Cargo.toml", "Cargo.lock", "clippy.toml", "rust-toolchain.toml"])`. `bazel/BUILD.bazel`: changelist's `fastbuild` `config_setting`.

`user.bazelrc.example`: changelist's, with the cache lines' comment naming the shared bound.

- [ ] **Step 3: Fetch**

Run: `bazelisk mod deps --lockfile_mode=update > target/task5.log 2>&1 && bazelisk query @crates//:all | head && bazelisk query @npm_web//:all | head -3 && bazelisk query @npm_pi//:all | head -3`
Expected: each query lists targets. `MODULE.bazel.lock` exists.

- [ ] **Step 4: Check for hard links in what Bazel wrote**

Run: `ob="$(bazelisk info output_base)"; find "$ob" -type f -links +1 | head`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add .bazelversion .bazelrc user.bazelrc.example .bazelignore .gitignore BUILD.bazel MODULE.bazel MODULE.bazel.lock bazel/BUILD.bazel web/BUILD.bazel plugins/pi/BUILD.bazel
git -c commit.gpgsign=false commit -m "Add the Bazel workspace: toolchains, crates from Cargo.lock, and each pnpm project's packages"
```

---

### Task 6: Derive Bazel's view of the Cargo workspace, and check it

**Files:**
- Create: `scripts/bazel-sync.sh`, `scripts/bazel-derive.py`, `bazel/workspace.bzl` (generated), `scripts/bazel-sync.test.sh`

**Interfaces:**
- Produces: `bazel/workspace.bzl` defining `CRATES`, a dict keyed by package path (`"crates/clax-server"`) with: `name`, `version`, `edition`, `lib` (crate name, underscores), `lib_root`, `bins` (`{name: root}`), `tests` (`{name: root}` from `[[test]]`), `default_features`, `deps` and `dev_deps` (workspace labels, e.g. `"//crates/clax-core:clax_core"`), `test_features` (`{package_path: [features]}`: features of workspace crates that this crate's dev-dependencies turn on, e.g. clax-mcp: `{"crates/clax-server": ["test-support"]}`; clax-server: `{"crates/clax-server": ["test-routes", "test-support"]}`), `has_build_script` (bool); and `WORKSPACE_RUSTC_FLAGS` (from `[workspace.lints.rust]`: `["-Funsafe-code"]`, plus `--check-cfg=cfg(clax_embedded)` from `unexpected_cfgs`).
- Produces: `scripts/bazel-sync.sh [--check]`; `--check` exits 1 naming `just bazel-sync` when any derived file is stale.

- [ ] **Step 1: Write the failing test**

`scripts/bazel-sync.test.sh`:

```bash
#!/usr/bin/env bash
# Tests scripts/bazel-sync.sh --check in a scratch copy of the repository's
# Cargo and Bazel files: current files pass; a new dependency, a changed
# workspace lint, a Rust version that differs from rust-toolchain.toml, a
# workspace crate without a BUILD.bazel, and a job limit in .bazelrc each
# fail, naming the cause; --check never writes a file.
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }
# A copy of every tracked and new (not ignored) file, as in the working tree.
fresh() {
    rm -rf "$T/r"; mkdir -p "$T/r"
    (cd "$HERE" && git ls-files -z --cached --others --exclude-standard | xargs -0 tar -cf - 2>/dev/null) | tar -xf - -C "$T/r"
}
# perl rather than sed -i: one spelling on macOS and Linux.
edit() { perl -0pi -e "$1" "$2"; }
check() { (cd "$T/r" && scripts/bazel-sync.sh --check) >"$T/out" 2>&1; }

fresh; if check; then pass "current files pass"; else fail "current files pass: $(cat "$T/out")"; fi

fresh; edit 's/^anyhow = "1"\n/anyhow = "1"\nitoa = "1"\n/m' "$T/r/Cargo.toml"
edit 's/^anyhow\.workspace = true\n/anyhow.workspace = true\nitoa.workspace = true\n/m' "$T/r/crates/clax-core/Cargo.toml"
before="$(cd "$T/r" && find . -type f -newer Cargo.toml | sort)"
if check; then fail "a new dependency fails"; elif grep -q 'just bazel-sync' "$T/out"; then pass "a new dependency fails, naming just bazel-sync"; else fail "message: $(cat "$T/out")"; fi
after="$(cd "$T/r" && find . -type f -newer Cargo.toml | sort)"
[ "$before" = "$after" ] && pass "--check writes nothing" || fail "--check wrote: $after"

fresh; edit 's/unsafe_code = "forbid"/unsafe_code = "deny"/' "$T/r/Cargo.toml"
if check; then fail "a changed lint fails"; else pass "a changed lint fails"; fi

fresh; edit 's/channel = "1.94.0"/channel = "1.95.0"/' "$T/r/rust-toolchain.toml"
if check; then fail "a toolchain mismatch fails"; elif grep -q 'rust-toolchain.toml' "$T/out"; then pass "a toolchain mismatch fails"; else fail "message: $(cat "$T/out")"; fi

fresh; rm "$T/r/crates/clax-hooks/BUILD.bazel"
if check; then fail "a crate without BUILD.bazel fails"; elif grep -q 'crates/clax-hooks' "$T/out"; then pass "a crate without BUILD.bazel fails"; else fail "message: $(cat "$T/out")"; fi

fresh; printf 'build --jobs=4\n' >> "$T/r/.bazelrc"
if check; then fail "a job limit fails"; else pass "a job limit in .bazelrc fails"; fi

exit "$FAILED"
```

- [ ] **Step 2: Run it to see it fail**

Run: `bash scripts/bazel-sync.test.sh`
Expected: FAIL (no `bazel-sync.sh`).

- [ ] **Step 3: Implement the sync**

Start from `../changelist/scripts/bazel-sync.sh` and `../changelist/scripts/bazel-derive.py`; drop the Wasm, release-server and binaryen derivations. `bazel-derive.py` reads `cargo metadata --locked --format-version 1` (with dependencies, to see which features each dev-dependency edge enables on a workspace crate) and writes `bazel/workspace.bzl` with the fields above, sorted, with a header `Generated by scripts/bazel-sync.sh from cargo metadata. Do not edit.` `tests` comes from each package's targets of kind `test`; `test_features` from each package's dev-dependency entries whose `path` is a workspace crate, unioned with that crate's normal-dependency features. `WORKSPACE_RUSTC_FLAGS` maps `[workspace.lints.rust]`: `forbid` → `-F<name with - for _>`, `deny` → `-D…`, `warn` → `-W…`, and `unexpected_cfgs`' `check-cfg` entries → `--check-cfg=<entry>`; any other lint table entry fails the sync with "add its mapping to scripts/bazel-derive.py".

`bazel-sync.sh` without `--check`: writes `bazel/workspace.bzl`, then `bazelisk mod deps --lockfile_mode=update`. With `--check`: compares a fresh derivation with the file; runs `bazelisk mod deps --lockfile_mode=error` to check the lockfile; compares `MODULE.bazel`'s `versions = [...]` with `rust-toolchain.toml`'s `channel`; checks every workspace member has a `BUILD.bazel`; fails if `.bazelrc` has a non-comment line with `--jobs`, `--local_resources` or `--local_cpu_resources`; and compares features: for each host triple, `cargo tree -e features --workspace` against `bazelisk cquery` of `@crates`' feature lists, failing on differences not listed in `NATIVE_EXTRA` in `bazel-derive.py` (each entry with a one-line reason; seed it from the first run, reviewing each). Every failure line ends with `run just bazel-sync and commit the result` where the sync fixes it.

- [ ] **Step 4: Generate and test**

Run: `scripts/bazel-sync.sh && bash scripts/bazel-sync.test.sh`
Expected: `bazel/workspace.bzl` written; all PASS.

- [ ] **Step 5: Commit**

```bash
git add scripts/bazel-sync.sh scripts/bazel-derive.py scripts/bazel-sync.test.sh bazel/workspace.bzl MODULE.bazel.lock
git -c commit.gpgsign=false commit -m "Derive Bazel's view of the Cargo workspace, its test targets, test features and lints, and check it"
```

---

### Task 7: rules_js and the web build

**Files:**
- Create: `bazel/js/BUILD.bazel`, `bazel/js/defs.bzl`, `bazel/js/npm_script.mjs`, `bazel/js/scratch.mjs`, `bazel/js/scratch.test.mjs` (copied from `../changelist/bazel/js/`, unchanged but for comments naming Clax's projects)
- Create: `bazel/dirs.bzl`
- Modify: `web/BUILD.bazel`, `plugins/pi/BUILD.bazel` (Task 5's stubs)

**Interfaces:**
- Consumes: `@npm_web`, `@npm_pi` (Task 5).
- Produces:
  - `npm_script_test(name, script, data, args = [], **kwargs)` and `npm_script_build(name, script, srcs, out_dirs, args = [], env = {}, **kwargs)` (changelist's signatures).
  - `pick_dir(name, src, dir)` in `bazel/dirs.bzl`: a target whose only file is the directory output of `src` whose path ends in `/<dir>`, so `$(execpath :name)` names one directory.
  - `//web:node_modules`, `//web:files`, `//web:dist_all` (`pnpm run build`, out_dirs `dist`, `dist-extension`, `dist-extension-test`), `//web:dist`, `//web:dist_extension`, `//web:dist_extension_test` (`pick_dir`s), `//web:dist_e2e` (`pnpm run build:e2e`, out_dirs `dist-e2e`).
  - `//plugins/pi:node_modules`, `//plugins/pi:files` (`copy_to_bin` of `src/**`, `skills/**`, `scripts/**`, `test/**`, `package.json`, `tsconfig.json`, `vitest.config.ts`, `README.md`), so a Pi file's real path sits beside `node_modules` in `bazel-bin/plugins/pi`.

- [ ] **Step 1: Write the failing check**

Run: `bazelisk build //web:dist`
Expected: FAIL (no package).

- [ ] **Step 2: Write `pick_dir`**

`bazel/dirs.bzl`:

```starlark
"""Picks one directory out of a target that writes several."""

def _pick_dir_impl(ctx):
    picked = [f for f in ctx.attr.src[DefaultInfo].files.to_list() if f.is_directory and f.short_path.endswith("/" + ctx.attr.dir)]
    if len(picked) != 1:
        fail("%s writes %d directories ending in /%s" % (ctx.attr.src.label, len(picked), ctx.attr.dir))
    return [DefaultInfo(files = depset(picked), runfiles = ctx.runfiles(files = picked))]

pick_dir = rule(
    implementation = _pick_dir_impl,
    attrs = {
        "src": attr.label(mandatory = True),
        "dir": attr.string(mandatory = True),
    },
    doc = "The one directory of `src` whose path ends in `/dir`, so `$(execpath)` names it.",
)
```

- [ ] **Step 3: Write `web/BUILD.bazel` and `plugins/pi/BUILD.bazel`**

`web/BUILD.bazel` follows `../changelist/web/BUILD.bazel`: `npm_link_all_packages(name = "node_modules")`; `copy_to_bin(name = "files", srcs = glob(["shell/**", "bridge/**", "extension/**", "contract/**", "e2e/**", "perf/**", "scripts/**"], exclude = ["**/.qual", "extension/key/key.pem"]) + ["package.json", "tsconfig.json", "svelte.config.js", "vite.shell.config.ts", "vite.bridge.config.ts", "vite.bridge-parts.config.ts", "vitest.config.ts", "playwright.config.ts", ".oxlintrc.json", "pnpm-workspace.yaml"])`; `npm_script_build(name = "dist_all", script = "build", srcs = [":files", ":node_modules"], out_dirs = ["dist", "dist-extension", "dist-extension-test"], visibility = ["//visibility:public"])`; the three `pick_dir`s, public; `npm_script_build(name = "dist_e2e", script = "build:e2e", srcs = [":files", ":node_modules", ":dist"], out_dirs = ["dist-e2e"])`.

`plugins/pi/BUILD.bazel`: `npm_link_all_packages(name = "node_modules")` and `copy_to_bin(name = "files", …)` as listed above, both public.

Wherever a bazel_lib copy rule in these files takes `hardlink`, pass `hardlink = "off"`.

- [ ] **Step 4: Build and compare with the native build**

Run: `bazelisk build //web:dist //web:dist_extension //web:dist_extension_test //web:dist_e2e //plugins/pi:node_modules //plugins/pi:files > target/task7.log 2>&1; tail -5 target/task7.log`
Expected: success.
Run: `scripts/build-web.sh >/dev/null && diff <(cd web/dist && find . -type f | sort) <(cd bazel-bin/web/dist && find . -type f | sort) && diff <(cd web/dist-extension && find . -type f ! -name .gitkeep | sort) <(cd bazel-bin/web/dist-extension && find . -type f | sort)`
Expected: no difference in file lists.
Run: `node bazel/js/scratch.test.mjs` (changelist's test of the runner's private directory).
Expected: PASS.
Run: `find "$(bazelisk info output_base)" -type f -links +1 | head`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add bazel/js bazel/dirs.bzl web/BUILD.bazel plugins/pi/BUILD.bazel
git -c commit.gpgsign=false commit -m "Build the web UI, the extension and the Pi package's files with Bazel, from each project's lockfile"
```

---

### Task 8: Rust libraries, binaries, lints and the warmed `clax`

**Files:**
- Create: `bazel/cargo.bzl`, `bazel/warm.bzl`, `crates/{clax-core,clax-server,clax-cli,clax-mcp,clax-hooks,clax-fake-exe}/BUILD.bazel`

**Interfaces:**
- Consumes: `CRATES`, `WORKSPACE_RUSTC_FLAGS` (Task 6); `//web:dist`, `//web:dist_extension` (Task 7); `CLAX_WEB_DIST_DIR`, `CLAX_EXTENSION_DIST_DIR`, `clax_embedded` (Task 3).
- Produces:
  - `cargo_package(compile_data = [], test_compile_data = [], test_data = [], tests_use_clax = [], test_env = {}, shard_counts = {}, build_script_env = {}, build_script_data = [])` in `bazel/cargo.bzl`, declaring per crate: `<lib>`, `<lib>_test` (only when another crate's `test_features` names this crate), `<lib>_lib_test`, `<lib>_doc_test`, each binary, `<bin>_bin_test`, each `[[test]]` (Task 9 fills the test attributes).
  - `warm_binary(name, binary)` in `bazel/warm.bzl`: an action that runs `binary --version` once and writes the binary's SHA-256 to `<name>.stamp`; tags `no-remote-cache`.
  - `//crates/clax-cli:clax`, `//crates/clax-cli:clax_warm`.

- [ ] **Step 1: Write the failing check**

Run: `bazelisk build //crates/...`
Expected: FAIL (no packages).

- [ ] **Step 2: Write `bazel/cargo.bzl`**

Start from `../changelist/bazel/cargo.bzl`. Keep `rustc_env` `CARGO_MANIFEST_DIR = package` and `CARGO_PKG_NAME`; add `CARGO_PKG_VERSION` from `crate["version"]`, `CARGO = "/nonexistent/cargo-under-bazel"` (spec §5.3), and `CLAX_BUILD_COMMIT = "unknown"` (B13). Every target gets `rustc_flags = WORKSPACE_RUSTC_FLAGS + ["--cfg", "clax_embedded"]`. Changes:

- Test targets come from `crate["tests"]`, each with `srcs = glob(["tests/**/*.rs"])` and `crate_root` its root; no globbing of `tests/*.rs` into targets (B8).
- When `crate["has_build_script"]`, declare `cargo_build_script(name = "build_script", srcs = ["build.rs"], build_script_env = {"CLAX_BUILD_COMMIT": "unknown"} | build_script_env, data = build_script_data, deps = all_crate_deps(build = True))` and add it to the library's and binaries' `deps`.
- `<lib>_test`: the library again with `crate_features` = default features + the features `test_features` names for it, and every test of a crate whose `test_features` names a workspace crate links that crate's `_test` variant instead of the plain library (B9).
- Debug information (Q7): workspace crates' targets get `-Cdebuginfo=line-tables-only` under `//bazel:fastbuild`; dependencies get none (rules_rust's default). Do not strip symbols.
- Drop changelist's `release`, `variant` and `_bin_exe` code.

- [ ] **Step 3: Write `bazel/warm.bzl`**

```starlark
"""Runs a built binary once, so macOS assesses it before any test runs it.

macOS assesses every new executable file on its first run, and Bazel makes a
new file for a binary in each output base. This action runs the binary (the
same file every test in this output base runs, through runfiles links) and
records its SHA-256. It is never taken from the disk cache (no-remote-cache),
so a worktree whose binary came from the cache still runs its own file once;
the output base's action cache skips it while the binary is unchanged.
"""

def _warm_binary_impl(ctx):
    bin = ctx.executable.binary
    stamp = ctx.actions.declare_file(ctx.label.name + ".stamp")
    ctx.actions.run_shell(
        inputs = [bin],
        outputs = [stamp],
        command = '"$1" --version >/dev/null && shasum -a 256 "$1" | cut -d" " -f1 > "$2"',
        arguments = [bin.path, stamp.path],
        mnemonic = "WarmBinary",
        progress_message = "Running %{input} once",
        execution_requirements = {"no-remote-cache": "1"},
    )
    return [DefaultInfo(files = depset([stamp]), runfiles = ctx.runfiles(files = [stamp, bin]))]

warm_binary = rule(
    implementation = _warm_binary_impl,
    attrs = {"binary": attr.label(mandatory = True, executable = True, cfg = "target")},
)
```

- [ ] **Step 4: Write the crates' BUILD files**

`crates/clax-core/BUILD.bazel`: `cargo_package(build_script_data = ["//web:extension_key"])`, with `web/BUILD.bazel` gaining `filegroup(name = "extension_key", srcs = glob(["extension/key/key.pub.b64"], allow_empty = True) + ["extension/key/.gitkeep"], visibility = ["//crates/clax-core:__pkg__"])` (whatever `web/extension/key/` holds in git; the build script reads the directory).
`crates/clax-server/BUILD.bazel`: `cargo_package(compile_data = ["//web:dist"], build_script_env = {"CLAX_WEB_DIST_DIR": "../../$(execpath //web:dist)"}, build_script_data = ["//web:dist"])` plus the `include_str!` inputs as `compile_data` (`//web:shell_cases`, a filegroup of `web/shell/src/*-cases.json` and the other files Task 3's includes name).
`crates/clax-cli/BUILD.bazel`: `cargo_package(compile_data = ["//web:dist_extension", "//plugins:embedded"], build_script_env = {"CLAX_EXTENSION_DIST_DIR": "../../$(execpath //web:dist_extension)"}, build_script_data = ["//web:dist_extension", "//plugins:embedded"])` and `warm_binary(name = "clax_warm", binary = ":clax", visibility = ["//visibility:public"])`; `plugins/BUILD.bazel` defines `filegroup(name = "embedded", srcs = glob(["claude-code/**", "clax/**", "clax-grok/**", "pi/src/**", "pi/skills/**", "pi/scripts/**", "pi/package.json", "pi/README.md"], exclude = ["**/node_modules/**", "**/.qual"]) + ["//:marketplaces"])` and the root `BUILD.bazel` a `filegroup(name = "marketplaces", srcs = [".claude-plugin/marketplace.json", ".agents/plugins/marketplace.json", ".grok-plugin/marketplace.json"])`.
The others: `cargo_package()`.

- [ ] **Step 5: Build, lint and format**

Run: `bazelisk build //crates/... > target/task8.log 2>&1 && bazelisk build --config=clippy //crates/... >> target/task8.log 2>&1 && bazelisk build --config=fmt //crates/... >> target/task8.log 2>&1; tail -5 target/task8.log`
Expected: all three succeed. Then put `unsafe {}` in a scratch line of `crates/clax-hooks/src/lib.rs`, run `bazelisk build //crates/clax-hooks:clax_hooks`, see it fail with `unsafe_code`, and undo the line.

- [ ] **Step 6: Check the binary and its warm-up**

Run:
```bash
bin=bazel-bin/crates/clax-cli/clax
"$bin" version --verbose            # expect: clax 0.4.0 / commit unknown
h="$(mktemp -d)"; CLAX_HOME="$h" "$bin" serve --foreground --bind 127.0.0.1 --port 0 & pid=$!
for _ in $(seq 100); do [ -f "$h/daemon.json" ] && break; sleep 0.1; done
port="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["port"])' "$h/daemon.json")"
curl -fsS "http://127.0.0.1:$port/" | grep -q '_clax/bridge.js?v=' && echo "embedded UI served"
kill "$pid"; wait "$pid" 2>/dev/null; rm -rf "$h"
bazelisk aquery 'mnemonic(WarmBinary, //crates/clax-cli:clax_warm)' | grep -q 'no-remote-cache' && echo "warm-up never cached"
```
Expected: `commit unknown`, `embedded UI served`, `warm-up never cached`. (The polling loop is a script's wait for a file with a bound, not a test.)

- [ ] **Step 7: Commit**

```bash
git add bazel/cargo.bzl bazel/warm.bzl crates/*/BUILD.bazel plugins/BUILD.bazel BUILD.bazel web/BUILD.bazel
git -c commit.gpgsign=false commit -m "Build every crate, its lints and format with Bazel, and run each new clax once before tests use it"
```

---

### Task 9: The Rust tests under Bazel

Implements the decisions on cda148bd (the warmed binary in the output base) and ede5fb4e (Cargo-like debug information); resolves the open half of 28025fcc (Bazel provides Node and Pi's packages to the real-client tests).

**Files:**
- Modify: `bazel/cargo.bzl` (test attributes), `crates/*/BUILD.bazel` (test data, `tests_use_clax`, shards)
- Modify: `crates/clax-server/tests/common/clients.rs:511, 689-697` (`CLAX_TEST_NODE`)
- Modify: `web/BUILD.bazel` (`filegroup(name = "contract_files", srcs = glob(["contract/**"]))`, visible to `//crates:__subpackages__`)
- Create: `scripts/BUILD.bazel` with `filegroup(name = "stable_bin", srcs = ["stable-bin.sh"])` and `filegroup(name = "test_scripts", srcs = ["ensure-clax.sh", "stable-bin.sh", "fake-exe.sh"])`, visible to `//crates:__subpackages__` (Task 11 adds the script tests)
- Create: `scripts/bazel-test.sh` (its `--rust` part), `scripts/bazel-parity.py`

**Interfaces:**
- Consumes: `clax_bin`, `repo` (Task 1); `CLAX_FAKE_EXE_DIR` (Task 2); `:clax`, `:clax_warm` (Task 8); `//plugins/pi:node_modules`, `//plugins/pi:files` (Task 7).
- Produces: `scripts/bazel-test.sh [--rust] [--web] [--ci] [bazel test args…]` (this task: `--rust`, the default); test environment `CLAX_TEST_BIN`, `CLAX_TEST_NODE`; `scripts/bazel-parity.py NEXTEST_LIST BAZEL_LOG_DIR`.

- [ ] **Step 1: Write the failing check**

Run: `bazelisk test //crates/... > target/task9.log 2>&1; grep -E '(PASSED|FAILED|NO STATUS)' target/task9.log | head`
Expected: no test targets, or failures for missing data.

- [ ] **Step 2: Give tests what they read**

In `cargo_package`, tests in `tests_use_clax` get `data += ["//crates/clax-cli:clax", "//crates/clax-cli:clax_warm"]` and `env = {"CLAX_TEST_BIN": "$(rootpath //crates/clax-cli:clax)"}`; every test gets `env["CLAX_TEST_NODE"] = "$(rootpath @nodejs_toolchains//:resolved_toolchain)"`-style Node path where the crate lists `//plugins/pi:node_modules` in `test_data` (use the label the Node toolchain exposes for its binary; `bazelisk query @nodejs_toolchains//...` lists it), and `timeout = "long"`. Shards: `shard_counts = {"integration": N}` sets `shard_count`; pick N from `rg -c '#\[(tokio::)?test' crates/clax-server/tests` so each shard holds about 100 tests. If rules_rust does not split a rust_test by `TEST_SHARD_INDEX`, wrap the test: `bazel/shard.sh` runs `"$TEST_BINARY" --list --format terse`, takes every `TEST_TOTAL_SHARDS`-th name from `TEST_SHARD_INDEX`, and runs them with `--exact`, touching `TEST_SHARD_STATUS_FILE`.

Per crate (`test_data`): `clax-server` — `glob(["tests/fixtures/**"])`, `//web:dist`, `//web:contract_files`, `//plugins/pi:node_modules`, `//plugins/pi:files`, `//scripts:stable_bin`; `clax-cli` — `//plugins:embedded`, `//scripts:test_scripts` (the scripts its tests run: `ensure-clax.sh`, `stable-bin.sh`, `fake-exe.sh`), `//web:dist_extension`; `clax-mcp`, `clax-hooks` — their fixtures; `clax-core` — `glob(["tests/**"])`. A test that fails with a missing file names it: add the file to the owning filegroup and rerun.

`crates/clax-server/tests/common/clients.rs`:

```rust
/// Node for the real Pi: `CLAX_TEST_NODE` when set (Bazel's toolchain,
/// relative to the start directory), else `node` on `PATH`.
fn node() -> Option<PathBuf> {
    std::env::var_os("CLAX_TEST_NODE")
        .map(|p| clax_fake_exe::bin::absolute_from(PathBuf::from(p), &clax_fake_exe::repo()))
        .or_else(|| which("node"))
}
```

and `run_pi` calls `node().expect("node: CLAX_TEST_NODE or PATH")`. `run_pi`'s `plugin` is `repo().join("plugins/pi")`; under Bazel its files are runfiles links into `bazel-bin/plugins/pi`, beside `node_modules`, so keep `.canonicalize()` only on `cli` and `ext` (the real files), never on the directory. If Pi's transpiler cache fails to write under the sandbox, set `JITI_CACHE_DIR` (or the variable the locked jiti reads; `rg -n 'CACHE' plugins/pi/node_modules/jiti/dist/*.js`) to the test's temporary directory in `run_pi`'s environment.

- [ ] **Step 3: Write `scripts/bazel-test.sh`**

Start from `../changelist/scripts/bazel-test.sh`: the `bazel-sync.sh --check` first, `caffeinate -i` on macOS, `_build`, `_pass`, `_summary`, a failing phase not stopping the next. Its Rust part: `_pass "rust tests" 'tests(//crates/...)' --test_env=CLAX_FAKE_EXE_DIR="${TMPDIR:-/tmp}/clax-fake-exe" --sandbox_writable_path="${TMPDIR:-/tmp}/clax-fake-exe" "$@"` (create the directory first). Drop the Postgres, Wasm and attention passes. Tasks 10–13 add `--web` and `--ci` phases.

- [ ] **Step 4: Run every Rust test**

Run: `scripts/bazel-test.sh --rust > target/task9.log 2>&1; tail -20 target/task9.log`
Expected: all targets PASSED.

- [ ] **Step 5: Check parity with nextest**

`scripts/bazel-parity.py` (adapted from changelist's): reads `cargo nextest list --workspace --message-format oneline` output and the `test.xml` files under `bazel-testlogs/crates/`, and fails listing any test name that ran under one and not the other.

Run: `cargo nextest list --workspace --message-format oneline > target/nextest.list && python3 scripts/bazel-parity.py target/nextest.list bazel-testlogs`
Expected: `parity: N tests, all in both`.

- [ ] **Step 6: Check no test paid a first run**

Run (macOS): `bazelisk clean && scripts/bazel-test.sh --rust > target/task9b.log 2>&1; grep -c WarmBinary target/task9b.log; grep -l 'CLAX_TEST_BIN .* missing\|timed out' bazel-testlogs/crates/**/test.log`
Expected: the warm action ran (count ≥ 1) and no log matches.

- [ ] **Step 7: Commit**

```bash
git add bazel/cargo.bzl bazel/shard.sh crates web/BUILD.bazel scripts/BUILD.bazel scripts/bazel-test.sh scripts/bazel-parity.py
git -c commit.gpgsign=false commit -m "Run every Rust test under Bazel, with the warmed clax, Node and Pi's packages as declared inputs"
```

Then reply on 28025fcc with the evidence (the passing `real Pi` tests under Bazel, and that the native gates still install `plugins/pi` first) and leave it to the owner to close.

---

### Task 10: Web and Pi checks

**Files:**
- Modify: `web/BUILD.bazel`, `plugins/pi/BUILD.bazel`, `scripts/bazel-test.sh` (`--web`)
- Create: `scripts/bazel-web.test.sh` (adapted from changelist's)

**Interfaces:**
- Produces: `//web:lint`, `//web:typecheck`, `//web:unit`, `//web:bundle_size`, `//plugins/pi:typecheck`, `//plugins/pi:test`.

- [ ] **Step 1: Write the failing test**

`scripts/bazel-web.test.sh` (changelist's, retargeted): on a private copy of the tree, deleting a module that `web/shell/src/clock.test.ts` imports makes `//web:unit` fail; deleting that test file removes it from the run; dropping `pako` from `web/pnpm-lock.yaml` makes `//web:bundle_size` fail to find it.
Run: `bash scripts/bazel-web.test.sh`
Expected: FAIL (targets missing).

- [ ] **Step 2: Add the targets**

`web/BUILD.bazel`: `npm_script_test(name = "lint", script = "lint", data = [":files", ":node_modules"])`, `typecheck` likewise, `unit` with `script = "test"`, and

```starlark
# The bundle gate: gzip sizes of the built web UI and extension against
# web/perf/bundle-budget.json (web/scripts/bundle-size.mjs). Its --record
# mode writes the budget file in the checkout and stays a native command.
js_test(
    name = "bundle_size",
    entry_point = "scripts/bundle-size.mjs",
    data = [":files", ":node_modules", ":dist", ":dist_extension"],
)
```

(if `bundle-size.mjs` reads `../dist` relative to itself, run it through `npm_script_test` with a `bundle-size` script added to `package.json` instead, so the runner's private copy holds `dist/` beside `scripts/`).

`plugins/pi/BUILD.bazel`: `npm_script_test(name = "typecheck", …)`, `npm_script_test(name = "test", script = "test", data = [":files", ":node_modules", "//crates/clax-cli:clax", "//crates/clax-cli:clax_warm"], env = {"CLAX_TEST_BIN": "$(rootpath //crates/clax-cli:clax)"}, args = ["--reporter=dot"])`. `daemon-fixture.ts` makes `CLAX_TEST_BIN` absolute against `process.cwd()` at load (`resolve(process.env.CLAX_TEST_BIN)`).

`bazel-test.sh --web`: `scripts/pnpm-lock-check.py web plugins/pi`, then `_pass "web and pi checks" 'tests(//web/... + //plugins/pi/... + //bazel/js/...) except attr(tags, "\bmanual\b", //web/...)'`, then `bash scripts/bazel-web.test.sh` last.

- [ ] **Step 3: Run them**

Run: `scripts/bazel-test.sh --web > target/task10.log 2>&1; tail -15 target/task10.log`
Expected: all pass, including `bazel-web.test.sh`.

- [ ] **Step 4: Commit**

```bash
git add web/BUILD.bazel web/package.json plugins/pi scripts/bazel-test.sh scripts/bazel-web.test.sh
git -c commit.gpgsign=false commit -m "Run the web UI's lint, type checks, unit tests and bundle gate, and the Pi extension's checks, under Bazel"
```

---

### Task 11: Script gates, scans, and no hard links anywhere Bazel writes

Implements the decision on 41e76eda.

**Files:**
- Create: `scripts/check-bazel-no-hard-links.sh`, `scripts/test-check-bazel-no-hard-links.sh`
- Modify: `scripts/BUILD.bazel` (Task 9's filegroups; add the tests)
- Modify: `scripts/check-no-hard-links.sh` (also scans `*.bzl`, `BUILD.bazel`, `MODULE.bazel`, `.bazelrc`; flags `hardlink = "on"` and `hardlink = "auto"`), `scripts/test-check-no-hard-links.sh` (cases for those), `scripts/test-release.sh` (its file list from a manifest), `scripts/bazel-test.sh` (`--ci`)

**Interfaces:**
- Produces: `sh_test`s `//scripts:{release,installer,plugin_wrapper,tool_hook,extension_signing,stable_bin,fake_exe,dev,comment_loop,no_unsafe,no_hard_links,pipefail_pipes,bazel_sync,run_choice}`; `scripts/check-bazel-no-hard-links.sh OUTPUT_BASE DISK_CACHE` (exit 1 listing any regular file with more than one link).

- [ ] **Step 1: Write the failing tests**

`scripts/test-check-no-hard-links.sh` gains: a scratch `BUILD.bazel` containing `copy_to_bin(name = "x", srcs = ["a"], hardlink = "auto")` fails the check; with `hardlink = "off"` it passes.
For `check-bazel-no-hard-links.sh`, add `scripts/test-check-bazel-no-hard-links.sh`, which makes no hard link: a scratch directory of plain files passes; then, with a fake `find` first on `PATH` (made with `scripts/fake-exe.sh`) that prints `<dir>/a` for `-links +1`, the check fails and names `<dir>/a`. The check calls `find` by name so the fake can stand in.
Run: `bash scripts/test-check-no-hard-links.sh; bash scripts/test-check-bazel-no-hard-links.sh`
Expected: FAIL.

- [ ] **Step 2: Implement the checks**

`scripts/check-bazel-no-hard-links.sh`:

```bash
#!/usr/bin/env bash
# Fails when Bazel's output base or the disk cache holds a regular file with
# more than one link: Clax makes no hard links, and neither may the rules it
# uses (docs/superpowers/specs/2026-10-15-bazel-overlay-design.md §13).
#
# Usage: scripts/check-bazel-no-hard-links.sh OUTPUT_BASE DISK_CACHE
set -euo pipefail
found="$(find "$@" -type f -links +1 2>/dev/null | head -20 || true)"
if [ -n "$found" ]; then
    echo "error: files with more than one link (hard links) under Bazel's directories:" >&2
    printf '  %s\n' "$found" >&2
    exit 1
fi
echo "no hard links in $*"
```

`scripts/test-release.sh`: move its `FILES` list into `scripts/test-release.files` (one path per line); the script reads it; `scripts/BUILD.bazel` declares `filegroup(name = "release_files", srcs = [<the same paths>])`, and `bazel-sync.sh --check` fails if the two lists differ.

- [ ] **Step 3: Declare the sh_tests**

`scripts/BUILD.bazel`: one `sh_test` per gate listed under Interfaces, each with `srcs` the gate's script and `data` the files it reads (its sourced helpers, `scripts/fake-exe.sh`, `scripts/fake-release-server.py`, `install.sh`, the plugins' wrappers, `:release_files`, and `//crates/clax-cli:clax` with `:clax_warm` plus `env = {"CLAX_TEST_BIN": "$(rootpath //crates/clax-cli:clax)"}` for `plugin_wrapper` and `comment_loop`). Each script that `cd`s to `"$(dirname "$0")/.."` already works from the runfiles tree. `run_choice` is Task 14's `run.test.sh`; declare it there.

`bazel-test.sh --ci` gains, in order: `scripts/bazel-sync.sh --check`, `_build "rust fmt check" --config=fmt //crates/...`, `_build "rust clippy" --config=clippy //crates/...`, the Rust pass, the script `sh_test`s (`_pass "script gates" 'tests(//scripts/...)'`), then the two gates run as they are, outside Bazel (spec §8): `scripts/test-plugins.sh` (reads git tags) and `scripts/test-justfile.sh` (runs `just`), and last `scripts/check-bazel-no-hard-links.sh "$(bazelisk info output_base)" ~/.cache/bazel-disk`.

- [ ] **Step 4: Run them**

Run: `bash scripts/test-check-no-hard-links.sh && bash scripts/test-check-bazel-no-hard-links.sh && bazelisk test //scripts/... > target/task11.log 2>&1; tail -20 target/task11.log`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add scripts
git -c commit.gpgsign=false commit -m "Run the script gates and source scans as Bazel tests, and fail on any hard link in Bazel's directories"
```

---

### Task 12: Browser tests with a pinned Chromium

**Files:**
- Create: `bazel/playwright/BUILD.bazel`, `bazel/playwright/browsers.bzl` (derived), `bazel/playwright/repo.bzl`
- Modify: `scripts/bazel-derive.py` and `scripts/bazel-sync.sh` (derive `browsers.bzl`), `MODULE.bazel` (`use_repo` of `chromium`), `web/BUILD.bazel`, `web/e2e/daemon-setup.ts`, `web/e2e/extension-fixtures.ts` (preset paths)

**Interfaces:**
- Produces: `@chromium//:browsers` (a directory laid out as `PLAYWRIGHT_BROWSERS_PATH` expects for the locked `@playwright/test`); `//web:playwright` (`js_binary` of the locked Playwright command line, for `install-deps` in CI); `//web:e2e_gesture`, `//web:e2e_chrome_overlay`, `//web:e2e_rest`, `//web:perf_usable`.

- [ ] **Step 1: Write the failing check**

Run: `bazelisk test //web:e2e_rest`
Expected: FAIL (no target).

- [ ] **Step 2: Pin Chromium**

`bazel-derive.py` reads the locked `playwright-core` version from `web/pnpm-lock.yaml`, its `browsers.json` (from `@npm_web`'s copy, through `bazelisk run` of a tiny `js_binary`, or by fetching the package tarball the lockfile names), and writes `bazel/playwright/browsers.bzl` with, per host platform (`mac-arm64`, `mac-x64`, `linux`, `linux-arm64`), the URLs and sha256 of the Chromium and Chromium headless-shell archives Playwright's own installer would download, and the directory names (`chromium-<revision>`, `chromium_headless_shell-<revision>`). `--check` fails when they differ from the lockfile's version. `repo.bzl` is a repository rule that downloads the host's archives by those checksums into that layout. Tag nothing to download at test time.

- [ ] **Step 3: Declare the browser targets**

`web/BUILD.bazel`: for each Playwright project, `npm_script_test(name = "e2e_<project>", script = "e2e", args = ["--project", "<project>"], data = [":files", ":node_modules", ":dist", ":dist_e2e", ":dist_extension_test", "//crates/clax-cli:clax", "//crates/clax-cli:clax_warm", "@chromium//:browsers"], env = {"CLAX_TEST_BIN": "$(rootpath //crates/clax-cli:clax)", "CLAX_E2E_WEB_DIST": "$(rootpath :dist_e2e)", "PLAYWRIGHT_BROWSERS_PATH": "$(rootpath @chromium//:browsers)", "CLAX_E2E_WORKERS": "8"}, tags = ["no-sandbox", "cpu:8"])`. The runner's private copy is where the test runs; make each path absolute in `global-setup.ts`/`daemon-setup.ts`/`extension-fixtures.ts` (`resolve(process.env.X)`) before use. `perf_usable`: `script = "perf"`, `env` with `CLAX_PERF_QUICK=1`, tags `["no-sandbox", "exclusive"]`; a `perf_usable_full` twin without `CLAX_PERF_QUICK`, tagged `manual`.

Try `//web:e2e_rest` once without `no-sandbox`. If Chromium starts, drop the tag on all three and record the result in `docs/bazel.md`; if not, keep it and record the error.

- [ ] **Step 4: Run them**

Run: `bazelisk test //web:e2e_gesture //web:e2e_chrome_overlay //web:e2e_rest > target/task12.log 2>&1; tail -10 target/task12.log` and then `bazelisk test //web:perf_usable >> target/task12.log 2>&1; tail -5 target/task12.log`
Expected: all pass; `bazelisk test //web:e2e_rest` a second time reports `(cached) PASSED`.

- [ ] **Step 5: Commit**

```bash
git add bazel/playwright scripts/bazel-derive.py scripts/bazel-sync.sh MODULE.bazel MODULE.bazel.lock web
git -c commit.gpgsign=false commit -m "Run the browser tests and time to usable under Bazel with a Chromium pinned by checksum"
```

---

### Task 13: Perf gates on an optimized build

Implements the decision on a2c067e5 (passed perf gates are cached).

**Files:**
- Create: `bazel/opt.bzl`
- Modify: `crates/clax-cli/BUILD.bazel`, `scripts/BUILD.bazel`, `scripts/perf-daemon.sh`, `scripts/perf-clients.sh` (accept `CLAX_PERF_BIN` relative to the start directory), `scripts/bazel-test.sh`

**Interfaces:**
- Produces: `opt_binary(name, binary)` (a transition to `compilation_mode = "opt"`, release profile settings: codegen units 16, no debug information, as `cargo build --release` with no `[profile.release]`); `//crates/clax-cli:clax_opt`, `//crates/clax-cli:clax_opt_warm`; `//scripts:perf_daemon`, `//scripts:perf_clients` (quick), `//scripts:perf_daemon_full`, `//scripts:perf_clients_full` (`manual`).

- [ ] **Step 1: Write the failing check**

Run: `bazelisk test //scripts:perf_daemon`
Expected: FAIL (no target).

- [ ] **Step 2: Implement**

`bazel/opt.bzl`: a rule with an outgoing transition setting `//command_line_option:compilation_mode` to `opt`, which copies its one input to its output with `ctx.actions.symlink` (no copy, no hard link) and is executable. `clax_opt_warm = warm_binary(binary = ":clax_opt")`. The perf `sh_test`s: `srcs` the script, `args = ["--quick"]` (none for `_full`), `data` the `.py` and budget files and `:clax_opt` with its warm stamp, `env = {"CLAX_PERF_BIN": "$(rootpath //crates/clax-cli:clax_opt)"}`, `tags = ["exclusive", "no-sandbox"]` (plus `manual` for `_full`). The scripts make `CLAX_PERF_BIN` absolute at start (`BIN="$(cd "$(dirname "$BIN")" && pwd -P)/$(basename "$BIN")"`).

`bazel-test.sh --ci` gains, after every other test: `_pass "perf gates" '//scripts:perf_daemon + //scripts:perf_clients + //web:perf_usable'` (Bazel runs exclusive tests one at a time, after the others).

- [ ] **Step 3: Run them, twice**

Run: `bazelisk test //scripts:perf_daemon //scripts:perf_clients //web:perf_usable > target/task13.log 2>&1; tail -8 target/task13.log; bazelisk test //scripts:perf_daemon 2>&1 | grep -q '(cached) PASSED' && echo cached`
Expected: all pass within budget; `cached`.

- [ ] **Step 4: Commit**

```bash
git add bazel/opt.bzl crates/clax-cli/BUILD.bazel scripts
git -c commit.gpgsign=false commit -m "Run the perf gates under Bazel on an optimized clax, alone, one at a time"
```

---

### Task 14: The overlay: `CLAX_BUILD`, `scripts/run.sh`, recipes and docs

**Files:**
- Create: `scripts/lib/build-system.sh`, `scripts/run.sh`, `scripts/run.test.sh` (adapted from changelist's three), `docs/bazel.md`
- Modify: `justfile`, `scripts/check.sh`, `scripts/BUILD.bazel` (`run_choice`), `AGENTS.md` ("Builds, tests and gates"), `docs/usage.md` ("Developing Clax")

**Interfaces:**
- Consumes: `scripts/bazel-test.sh` (Tasks 9–13), `scripts/quality_gates.sh`.
- Produces: `scripts/run.sh [--dry-run] <ci|check|test|lint|web-test|pi-test|web-e2e|perf|build|web|which> [args]`; `CLAX_BUILD`; recipes of spec §4.2 and their `-native` twins.

- [ ] **Step 1: Write the failing test**

`scripts/run.test.sh` (from changelist's): on a `PATH` of fake tools made with `scripts/fake-exe.sh` (`bazelisk`, `bazel` reporting a version, `cargo`, `node`, `pnpm` that record their arguments), check:
- Bazelisk present, `CLAX_BUILD` unset → `ci` runs `scripts/bazel-test.sh --ci`, and prints no native line.
- No Bazel → `ci` runs `scripts/quality_gates.sh`; first line names Bazelisk and its install command; last line says the result came from the native tools and CI's can differ; exit status is the gates' own.
- A `bazel` of another version only → native, with the reason.
- `CLAX_BUILD=bazel` without Bazel → exit 2 with the install command; nothing runs.
- `CLAX_BUILD=native` with Bazel → native, and no line about installing Bazel.
- `which` prints `bazel: …` or `native: …`; `--dry-run test` prints `would run: …` and runs nothing.
- `web` with Bazel → after the fake build, `web/dist` and `web/dist-extension` in the scratch checkout hold copies of the fake `bazel-bin/web/dist` files: regular files (`[ ! -L f ]`), writable (`[ -w f ]`), link count 1 (`stat -f %l` / `stat -c %h` is `1`) (Review Focus 5).
Run: `bash scripts/run.test.sh`
Expected: FAIL (no `run.sh`).

- [ ] **Step 2: Implement**

`scripts/lib/build-system.sh`: changelist's, with `CLAX_BUILD`, needs `rust`/`web` checking `cargo`, `node`, `pnpm`. `scripts/run.sh`: changelist's structure; `_bazel`: `ci` → `scripts/bazel-test.sh --ci`; `check` → `cargo fmt --all` (when `cargo` exists) then `bazel-test.sh --ci`; `test` → `bazel-test.sh --rust`; `lint` → `bazelisk build --config=fmt //crates/... && bazelisk build --config=clippy //crates/... && bazelisk test //web:lint`; `web-test` → `bazelisk test //web:lint //web:typecheck //web:unit //web:bundle_size`; `pi-test` → `bazelisk test //plugins/pi:typecheck //plugins/pi:test`; `web-e2e` → `bazelisk test //web:e2e_gesture //web:e2e_chrome_overlay //web:e2e_rest`; `perf` → the `_full` perf targets; `build` → `bazelisk build //crates/clax-cli:clax`; `web` → `bazelisk build //web:dist //web:dist_extension`, then for each directory `rm -rf` the files under `web/<dir>` except `.gitkeep` and copy with `cp -R`, then `chmod -R u+w`. `_native`: today's recipe bodies (`quality_gates.sh`, `check.sh`, the nextest block of `just test`, etc.). Both paths wrap runs in `caffeinate -i` on macOS.

`justfile`: each recipe of spec §4.2 calls `./scripts/run.sh <task> {{ARGS}}`, and each gets a `-native` twin `CLAX_BUILD=native ./scripts/run.sh <task> {{ARGS}}`; add `bazel-sync` and `bazel-gc` recipes. Every recipe keeps a description (`scripts/test-justfile.sh` checks it). `scripts/check.sh` keeps its native body; `run.sh check` decides.

`docs/bazel.md`: changelist's structure, for Clax: Use (the table of what `auto` picks; the "Instead of / Run" table), Keeping Bazel in step with Cargo, targets per crate, the macOS first-run section (spec §6), the web and Pi section, which suites run outside Bazel and why (spec §8), CPU and memory, CI (filled by Task 16), Disk and Speed (filled by Task 17), For agents, Differences from Cargo (spec §16). `AGENTS.md` and `docs/usage.md` point to it and say: run checks through the recipes; report a native result as native; `just bazel-sync` after any `Cargo.toml`/`Cargo.lock` change; `just bazel-gc` after removing worktrees.

- [ ] **Step 3: Run the tests and the whole suite both ways**

Run: `bash scripts/run.test.sh && bash scripts/test-justfile.sh`
Expected: PASS.
Run: `just ci > target/task14.log 2>&1; tail -5 target/task14.log; just ci-native > target/task14n.log 2>&1; tail -5 target/task14n.log`
Expected: both pass; the native run's last line names the native tools.

- [ ] **Step 4: Commit**

```bash
git add scripts/lib/build-system.sh scripts/run.sh scripts/run.test.sh scripts/BUILD.bazel scripts/check.sh justfile docs/bazel.md AGENTS.md docs/usage.md
git -c commit.gpgsign=false commit -m "Run every check and build with Bazel when it is installed and natively otherwise, saying which, with a native twin for each recipe"
```

---

### Task 15: `just bazel-gc` and the shared bound

Implements the decision on caa4d4f2; leaves Q10 (thread 7c03cf06) to the owner.

**Files:**
- Create: `scripts/bazel-gc.sh`, `scripts/bazel-gc.test.sh` (adapted from changelist's)
- Modify: `scripts/bazel-sync.sh` (`--check` compares `.bazelrc`'s three cache lines with what `bazel-gc.sh` reads), `scripts/BUILD.bazel` (`bazel_gc` sh_test), `justfile`

**Interfaces:**
- Produces: `scripts/bazel-gc.sh [--dry-run] [--report] [--adopt DIR]`; it reads the cache path, size and age from `.bazelrc`'s `--disk_cache`, `--experimental_disk_cache_gc_max_size`, `--experimental_disk_cache_gc_max_age` lines (`BAZEL_GC_MAX_SIZE_GB`/`BAZEL_GC_MAX_AGE_DAYS` override, whole numbers above zero); accepts a cache marked by either `.clax-bazel-cache` or changelist's `.changelist-bazel-cache`.

- [ ] **Step 1: Write the failing test**

`scripts/bazel-gc.test.sh` (changelist's, plus):
- A scratch checkout whose `.bazelrc` says 40G, beside a scratch `../changelist/.bazelrc` saying 20G: `bazel-gc.sh --dry-run` prints a warning naming `40` and `20` and both files; with both at 40G, no warning.
- The bound comes from `.bazelrc`: with `--experimental_disk_cache_gc_max_size=3G` and 5 GB of fake entries (sparse files are not enough: write real 1 MB files and set `BAZEL_GC_UNIT_BYTES=1048576` so "GB" means MB in the test), oldest-first deletion stops under the bound.
- A directory marked `.changelist-bazel-cache` is trimmed; an unmarked one is refused.
- An output base whose workspace directory is gone and whose `server/server.pid.txt` names a dead PID is removed; one whose PID is alive is kept; nothing is found by `pgrep`.
- `--report` prints real sizes of the cache, the output bases and the downloads.
Run: `bash scripts/bazel-gc.test.sh`
Expected: FAIL.

- [ ] **Step 2: Implement**

Adapt changelist's `scripts/bazel-gc.sh`: bounds read from `.bazelrc` as above; output bases found under `bazelisk info output_user_root`; a base is Clax's when its `README`/workspace link names a directory inside this repository's main checkout (`git rev-parse --git-common-dir`'s parent) or its worktrees; servers judged alive by `kill -0` on their PID file; the sibling check reads `"$(git rev-parse --show-toplevel)/../changelist/.bazelrc"` and, for the main checkout's worktrees, the main checkout's sibling. The `BAZEL_GC_UNIT_BYTES` variable exists only for the test and defaults to 1073741824.

- [ ] **Step 3: Run it**

Run: `bash scripts/bazel-gc.test.sh && scripts/bazel-gc.sh --dry-run --report`
Expected: tests pass; the real run lists what it would remove and prints sizes, and warns that changelist states 20 GB until Q10 is settled.

- [ ] **Step 4: Commit**

```bash
git add scripts/bazel-gc.sh scripts/bazel-gc.test.sh scripts/bazel-sync.sh scripts/BUILD.bazel justfile
git -c commit.gpgsign=false commit -m "Free what Bazel leaves behind, under the bound .bazelrc states for the cache shared with changelist"
```

---

### Task 16: CI on Bazel

Implements the decisions on 8fa66311 (GitHub-hosted runners with `actions/cache`) and 10092cfd (CI gates on Bazel only; the native gate is manual).

**Files:**
- Create: `.github/actions/setup-bazel/action.yml`, `.github/workflows/native-check.yml`
- Modify: `.github/workflows/ci.yml`, `.bazelrc` (`ci` config), `docs/bazel.md` ("CI")

**Interfaces:**
- Consumes: `scripts/bazel-test.sh --ci` (Tasks 9–13).
- Produces: CI job `bazel` gating pull requests and pushes to main.

- [ ] **Step 1: Write the composite action**

`.github/actions/setup-bazel/action.yml`: inputs `save` (boolean). Steps: install Bazelisk (pinned release, by checksum); `actions/cache/restore` (pinned by commit SHA) of `~/.cache/bazel-disk` and `~/.cache/bazel-repo` with key `bazel-${{ runner.os }}-${{ hashFiles('.bazelversion', 'MODULE.bazel.lock', 'Cargo.lock', 'web/pnpm-lock.yaml', 'plugins/pi/pnpm-lock.yaml') }}-${{ github.sha }}` and restore-keys `bazel-${{ runner.os }}-${{ hashFiles(…same…) }}-` then `bazel-${{ runner.os }}-`; write `user.bazelrc` with `common --config=ci`.
`.bazelrc` `ci` config: `common:ci --repository_cache=~/.cache/bazel-repo`, `common:ci --lockfile_mode=error`, `build:ci --experimental_disk_cache_gc_max_size=6G`, `test:ci --test_timeout=300`.

- [ ] **Step 2: Rewrite `ci.yml`**

```yaml
name: ci
on: { push: { branches: [main] }, pull_request: {} }
permissions: { contents: read }
jobs:
  bazel:
    runs-on: ubuntu-latest
    timeout-minutes: 60
    steps:
      # The full history and its tags: scripts/test-plugins.sh checks the
      # plugins' pinned release against the newest v* tag.
      - uses: actions/checkout@<sha> # v4
        with: { fetch-depth: 0, persist-credentials: false }
      - uses: ./.github/actions/setup-bazel
      # Only for `cargo metadata` in scripts/bazel-sync.sh --check; Bazel
      # fetches its own compiler, Node and packages.
      - uses: dtolnay/rust-toolchain@<sha>
        with: { toolchain: 1.94.0 }
      - uses: extractions/setup-just@<sha> # scripts/test-justfile.sh
      # Chromium's system libraries; the browser itself comes from Bazel.
      - run: bazelisk run //web:playwright -- install-deps chromium
      - run: scripts/bazel-test.sh --ci
      - if: ${{ !success() }}
        uses: actions/upload-artifact@<sha> # v4
        with:
          name: bazel-testlogs
          path: |
            bazel-testlogs/**/test.log
            bazel-testlogs/**/test.xml
            bazel-testlogs/web/**/test.outputs/**
          retention-days: 14
      - name: Save the cache (main only, after every step passed)
        if: ${{ success() && github.event_name == 'push' && github.ref == 'refs/heads/main' }}
        uses: actions/cache/save@<sha>
        with: { path: "~/.cache/bazel-disk\n~/.cache/bazel-repo", key: "<the same key as the restore>" }
      - if: always()
        run: bazelisk shutdown || true
```

Pin each action by full commit SHA with its version in a comment, as changelist's workflows do. `native-check.yml`: `on: workflow_dispatch`, today's `ci.yml` job (with Task 4's pnpm setup) running `scripts/quality_gates.sh`.

- [ ] **Step 3: Check the workflows**

Run: `actionlint .github/workflows/*.yml .github/actions/setup-bazel/action.yml` (install with `brew install actionlint` if missing)
Expected: no findings.
The branch's own CI run (once the coordinator pushes it) must pass, and a re-run must restore the cache (the job log's restore step says `Cache restored`). Record both run URLs in `docs/bazel.md`.

- [ ] **Step 4: Commit**

```bash
git add .github .bazelrc docs/bazel.md
git -c commit.gpgsign=false commit -m "Gate CI on Bazel with a cache restored from main, and keep the native checks as a manual workflow"
```

---

### Task 17: Measure, set the cache bound, and meet the merge criteria

Implements the decision on 2f40c94c (speed wins). This task is the merge gate (spec §15); anything that fails here is fixed before merge, in the task that owns it.

**Files:**
- Modify: `docs/bazel.md` ("Disk", "Speed", the gate-for-gate table), `.bazelrc` and `user.bazelrc.example` (the measured bound), `scripts/bazel-gc.sh` defaults if they change
- Modify: `docs/superpowers/specs/2026-10-15-bazel-overlay-design.md` §9.3 (the measured numbers, replacing the estimate)

- [ ] **Step 1: Speed, on an idle machine (spec §10.1)**

For each row, run three times and keep the median; run native and Bazel interleaved:

| Case | Bazel | Native |
|---|---|---|
| no change since the last run | `just ci` | `just ci-native` |
| fresh worktree at a commit another worktree built (`git worktree add`, then the recipe) | `just ci` | `just ci-native` |
| after an edit to `crates/clax-core/src/lib.rs` (a comment), to `crates/clax-server/src/routes/shell.rs`, to `web/shell/src/clock.ts` | `just ci` | `just ci-native` |
| after the clax-core edit | `just test` | `just test-native` |
| after the `web/shell` edit | `just web-test` | `just web-test-native` |

Expected: every Bazel row under its budget (20 s, 60 s, 2 min) and no slower than its native twin. A row that misses fails this task: profile it (`--profile=target/bazel.prof`, `bazelisk analyze-profile`), fix it in the owning task's files (the levers are spec §10.2's), and measure again.

- [ ] **Step 2: First runs (spec §6.4)**

In a second worktree at the same commit: `scripts/bazel-test.sh --rust` must show the `WarmBinary` action run there, and no test log may show a first exec of `clax` over 1 s (the helper of Task 1 logs the first spawn's duration when `CLAX_TEST_LOG_FIRST_EXEC=1`; add that one line there if it is missing). Then the same while a third worktree runs `bazelisk build //...`.

- [ ] **Step 3: Disk (spec §9.2, §9.3)**

Record, with `scripts/bazel-gc.sh --report` and `du -sk` of the cache's `cas` and `ac` entries newer than the run's start: what one build of main adds to `~/.cache/bazel-disk`; what one edit of each crate and of `web/shell` adds; a fresh worktree's output base's real disk (use `df` before and after, since clones are not counted by `du`); an edited worktree's. Set the shared bound to changelist's measured 7-day working set (14 GB on 2026-10-15, re-measured) plus main's build plus five edited worktrees, rounded up to 5 GB; update `.bazelrc`, `user.bazelrc.example` and spec §9.3 with the numbers and the bound. Expected: fresh worktree under 0.5 GB, edited under 3 GB.

- [ ] **Step 4: The rest of the merge criteria**

- `python3 scripts/bazel-parity.py …` (Task 9 Step 5): all in both.
- `scripts/check-bazel-no-hard-links.sh "$(bazelisk info output_base)" ~/.cache/bazel-disk`: none.
- `just ci-native`: passes.
- The gate-for-gate table in `docs/bazel.md`: every `quality_gates.sh` gate has its Bazel target, or its §8 reason.
- CI on the branch: passed, and a re-run restored the cache (Task 16).
- `qualifier threads --status needs-decision` lists only Q10 (7c03cf06) and the owner confirmations on the spec's threads.

- [ ] **Step 5: Commit**

```bash
git add docs/bazel.md .bazelrc user.bazelrc.example scripts/bazel-gc.sh docs/superpowers/specs/2026-10-15-bazel-overlay-design.md
git -c commit.gpgsign=false commit -m "Record Bazel's measured speed and disk use, and set the shared cache's bound from them"
```

---

## Threads

| Thread | What | Task |
|---|---|---|
| b42d4ebf | a bare `cargo nextest run` races on `target/debug/clax` | 1 (closes, with evidence) |
| 28025fcc | real-client tests need Node and Pi's packages | 9 (reply with evidence; the owner closes) |
| d95eeace | Q1 pnpm | 4 |
| caa4d4f2 | Q2 shared cache | 5, 15, 17 |
| 8fa66311 | Q3 CI runner and cache | 16 |
| cda148bd | Q4 warmed test binary | 8, 9 |
| 2f40c94c | Q5 speed wins | 17 |
| a2c067e5 | Q6 cached perf | 13 |
| ede5fb4e | Q7 debug information | 8 |
| 41e76eda | Q8 hard links in Bazel's directories | 11 |
| 10092cfd | Q9 CI on Bazel only | 16 |
| 7c03cf06 | Q10 changelist's side of the bound | not done here: the owner decides; Task 15 warns while the bounds differ |
| 39562e11 | shell unit tests may join the lanes in `quality_gates.sh` | not done here: native-gate scheduling, unchanged by this plan |
| 0f42bd1c, e838ac28 | timing in `clax-hooks` golden tests | not done here; Task 9 reruns them under Bazel and reports any failure on those threads |

The decision threads stay open until the owner confirms the recorded decisions; implementers do not close them.
