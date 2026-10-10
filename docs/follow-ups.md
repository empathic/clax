# Known issues and follow-ups

Follow-ups that concern specific code (known issues, deferred fixes,
flaky tests, paths not taken, and questions for the owner) live in `.qual`
files next to that code, written with
[qualifier](https://github.com/empathic/qualifier) (see `AGENTS.md`).
List them with `qualifier threads`, the ones under a path with
`qualifier threads <path>`, and the questions waiting on the owner with
`qualifier threads --status needs-decision`.

This file keeps what has no code to pin: checks only the owner can run,
and project-wide notes. Remove an entry when it is done or decided.

## Checks only the repository owner can run

These touch a real harness, a real home or GitHub, or need a person at the
machine, so no agent runs them.

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
- **Consequential actions with a real screen reader** (security pass).
  Send, Resolve, Reply, the batch sends, Post in a composer the page opened,
  and the consent dialog's Allow act only on a trusted click whose `detail`
  is 1 or more once the keyboard trail is tainted, which is every load.
  Check with VoiceOver in Safari and Chrome whether its activation clicks
  with `detail` 1 (it acts) or 0 (it says "Click to <verb>" and acts
  only on a pointer), and record which in `docs/contract.md` "Known
  limitations".
- `scripts/smoke-claude-push.sh --channel` and `--follow`: an idle Claude
  Code session wakes on a comment through the channel and through the
  background follow fallback. Until the owner runs them, two facts are
  unverified live: that a channel event from `plugin:clax@clax` starts a
  turn, and that a background Bash command's exit starts one in an idle
  session.
- **Unsigned commits.** The commits from 2e08cad through the end of the
  stable-install work were made unsigned, for one batch re-sign later.

## Grok Build

From the Grok Build plan (`docs/superpowers/plans/2026-10-01-grok-build.md`).
Its provisional answers to Q1, Q2, Q3 and Q5 are recorded as `alternative`
records on the code each would change.

- **The live checks** (Q4), which only the owner runs, with
  `scripts/smoke-grok.sh` and a real Grok TUI. Only a fake Grok
  (`crates/clax-cli/tests/grok_dedupe.rs`) has run them so far:
  - a: `grok plugin install <dir> --trust` and `grok plugin uninstall
    clax-grok --confirm` take those arguments, and install copies the plugin;
  - b: `grok plugin list --json` names an installed plugin's source path;
  - c: stdio MCP servers get `GROK_SESSION_ID`, and hooks get
    `GROK_HOOK_EVENT` and `sessionId`;
  - d: a Stop hook's `{"decision":"block"}` continues the turn, the next Stop
    has `stopHookActive: true`, and the session-end Stop's `reason` is not
    `end_turn`;
  - e: servers `clax` and `clax_grok` from two enabled plugins both load, and
    `search_tool` finds `clax_grok__publish`;
  - f: `/new` and `/resume` start a new MCP server with the new
    `GROK_SESSION_ID`;
  - g: the `SessionEnd` hook's `timeout: 2` is honoured, or capped at 1.5 s;
  - the monitor wake (h, i): a persistent `monitor` running `clax feedback
    follow` wakes an idle TUI on each line and a busy one after its turn, and
    the agent can build its command from `clax_grok__status`.

  Then record the measured Grok version in `docs/contract.md` and drop "not
  yet run live".

## Gate speed

`scripts/quality_gates.sh` runs in about 110 s on a warm cache and an
otherwise idle machine: the web build and unit tests (about 10 s), then the
lanes with web e2e beside the rest (about 55 s), then the perf gates alone
(about 41 s). The slow tests and gate choices that concern specific code are
records tagged `gate-speed` (`qualifier threads --tag gate-speed`). What
is left beyond them:

- **macOS assesses each new executable before it first runs.** Gatekeeper
  scans every unsigned executable a process tree runs for the first time
  (`GK performScan` in syspolicyd's log): an XProtect analysis and an online
  notarization lookup, about 0.15 s for a small script and 1.35 s for the
  80 MB debug `clax`. The verdict is kept per process tree, and cargo and
  nextest each start a new one, so every Rust test that runs `clax` pays the
  1.35 s once (about 150 tests: most of the 260 s the Rust tests add up to).
  syspolicyd handles these one at a time, so the lanes wait on each other
  through it while the CPU stays mostly idle (2 to 3 of 12 cores in use),
  and on a loaded machine a first run takes far longer: 1.5 to 14 s for a
  two-line script during a gates run, against 10 ms for its second run.
  That is longer than the limits the code under test puts on the programs
  it starts (2 to 5 s), so the tests' stand-in executables are never on a
  first run when it counts: `crates/clax-fake-exe`, `scripts/fake-exe.sh`
  and `plugins/pi/test/fake-exe.ts` keep one read-only copy of each script
  text, run it once outside any limit and put a symbolic link to it where a
  test wants it (the verdict belongs to the file: a new file with the same
  text is scanned again, a run through a symbolic link to a scanned one is
  not), or, for a fake the code under test identifies by its file (an
  installed `clax`, a daemon's executable), write a file of its own and run
  it once before renaming it into place; the gates and `just test` keep the
  `clax` they copy under `target/clax-bin`, by its content
  (`scripts/stable-bin.sh`), so a copy is new, and run once, only after a
  rebuild; the gates run each `clax` they build once, as soon as it exists;
  and the plugin
  wrapper test, whose managed installs run freshly extracted binaries, runs
  copies of the wrapper with long limits, and short ones only in the cases
  about the limits. Terminals listed under System Settings, Privacy & Security,
  Developer Tools are not assessed; that is a setting of the machine, not
  of the repository. A smaller debug binary would shorten each scan
  (`opt-level = 1` gives 32 MB and 0.76 s, at twice the cold build time).
