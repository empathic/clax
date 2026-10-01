# Known issues and follow-ups

Open items from the stable-install work (plan
`docs/superpowers/plans/2026-09-30-stable-install.md`), with where each was
found. Remove an entry when it is fixed or decided.

## Checks only the repository owner can run

These touch a real harness, a real home or GitHub, so no agent runs them.

- **`just dev claude`, `just dev pi` and `just dev codex` against the real
  CLIs.** Only fake harness commands have run these loops. Run each once and
  check the session sees the dev build and `~/.clax-dev`.
- **`clax init` and `clax uninit` against the real harness CLIs.** Only fake
  `claude`, `codex` and `pi` commands have exercised them (Task 6 skipped its
  real-CLI step). Run `scripts/verify-harnesses.sh` and keep its whole
  output (plan, "Steps for the person", A).
- **Pi loading the installed copy.** `clax init` registers
  `~/.clax/marketplace/plugins/pi`, which has no `node_modules`. Whether a
  real Pi resolves `typebox` there is unchecked (plan, B).
- **The MCP fallback in a real harness.** A harness started with no `clax` on
  `PATH` should show the `clax` server with the single tool `status` (plan,
  B).
- **No model-driven Pi session** has been run end to end (see "What is not
  yet available" in `docs/contract.md`).
- **The Codex plugin validator** has never run: `scripts/test-plugins.sh`
  skips it when it is not installed.
- **The release workflow on GitHub** (Task 9). Only a real run proves:
  - that the runner labels `macos-15-intel` and `ubuntu-24.04-arm` exist;
  - the `x86_64-apple-darwin` and musl Linux builds, and their smoke steps;
  - the artifact merge into `release-dist`;
  - `gh release create` in the publish job.

  `actionlint` was not run either (not installed). After a real release, and
  only once the repository is public, `install.sh` from GitHub is still
  untested (plan, C).
- **Unsigned commits.** The commits from 2e08cad through the end of the
  stable-install work were made unsigned, for one batch re-sign later.

## Decisions still open

- **Grok Build.** The plugin's name (`clax-grok` is the recommendation) and
  the duplicate-plugin guard (Grok also discovers the Claude Code plugin) are
  proposals, not decisions. Grok support is queued after the stable install.

## Known issues

- **The daemon's own `/mcp` `status` does not report `upgrade_held`.** The
  shim, the CLI and Pi report it; the daemon-served MCP endpoint has no hold
  probe (Task 5 review, Low 5).
- **One canonicalization in the daemon replacement has no test.** Reverting
  the second canonical-path comparison in `crates/clax-cli/src/client.rs`
  passes every test; the first one, in `replace`, is tested and covers the
  same case (Task 3 re-review 2).
- **`clax doctor --agent`'s `binary` check probes differently from the
  wrapper** (Task 5 review, Low 1). Doctor, and Pi's `binaryVersion`, allow
  3 s for `--version` and ignore its exit status; the wrapper allows 5 s and
  needs exit 0. A `clax` whose first run takes 3 to 5 s is run by the wrapper
  but called "not clax" by doctor. Fix: use the wrapper's rule.
- **"(not clax)" covers four causes** (Task 5 review, Low 2): a foreign
  program, a file that is not executable, a timeout, and a crash. Say which,
  as the wrapper does.
- **The `binary` advice ignores `CLAX_BIN`** (Task 5 review, Low 3). With
  `CLAX_BIN` set, doctor does not check that binary and still advises
  `just install`. It should report whether `CLAX_BIN` is a usable clax and
  advise unsetting it or pointing it at this binary.
- **doctor's `version_line` can block** (Task 5 review, Low 4). It reads
  stdout after the child exits, so a `--version` that leaves a background
  process holding stdout waits past the timeout, and on timeout only the
  direct child is killed. Fix: read on a thread with the deadline, or kill a
  process group.
- **A held upgrade's `reason` goes stale** (Task 5 review, Nit 8). It keeps
  the PID and port of the rolled-back daemon at the time of failure, which
  `status` and doctor print later as `why:`.
- **The wrapper's probes plus the daemon start can exceed Codex's MCP
  startup timeout** in pathological cases (Task 4 re-review 1): up to 5 + 5 s
  of probes, then up to 5 s for a daemon start or longer for a replacement,
  before the shim answers `initialize`. Options: one shared deadline for the
  probes, or a slow-probe line in `hooks.log`.
- **`scripts/verify-harnesses.sh` usability** (Task 6 re-review 4):
  - N4-1: the harness versions are printed only at the top, not above the
    table, and nothing says what to paste. Until fixed, keep the whole
    output.
  - N4-2: a failed seed step reports one line, and its log is deleted with
    the scratch root.
  - N4-N1: the watchdog sets the process group only in the child; a signal
    in the microseconds after `fork` can make it wait for the command.
  - N4-N2: a CLI that reads `/dev/tty` is stopped by SIGTTIN and then
    reported as a timeout.
  - N4-N3: the docs say each step is killed "with everything it started";
    a descendant that calls `setsid` survives. Say "with its process group".
  - N4-N4: an exported `CLAX_BIN` skips the build with no note beyond the
    header.
  - N4-N5: a second Ctrl-C during cleanup can leave the scratch root behind;
    ignore signals at the top of `cleanup`.

## Tests

These tests have failed intermittently under heavy machine load and passed
on rerun; each is parked for a fix:

- `clax-mcp` `open_status`: relies on a 1.5 s wait for the fake browser opener.
- `api_docs::an_oversized_batch_names_the_docs_batch_limit`: a connection reset
  under load.
- `web/shell/src/artifact.test.ts`, "says so when the page of an opened thread
  never greets": races a 50 ms wait against 120 ms sleeps.
- `web/e2e/subpages.spec.ts`, "sandbox: one link inside the frame is one
  history entry": failed about 1 run in 120 under load; passed 10 of 10 alone.
- `web/e2e/artifact.spec.ts`, "sandbox: a second viewer's vote 2 s after
  another viewer's vote reloaded it publishes": the reloaded frame did not
  appear within 30 s once in a full gates run; passed 10 of 10 alone.

`scripts/quality_gates.sh` takes a lock per checkout and is read whole before
it runs, so concurrent runs and mid-run edits no longer break it.

