# Known issues and follow-ups

Open items from the stable-install work (plan
`docs/superpowers/plans/2026-09-30-stable-install.md`), the Svelte port
(plan `docs/superpowers/plans/2026-09-29-svelte-port.md`) and Echo (plan
`docs/superpowers/plans/2026-09-30-agent-working.md`), with where each was
found. Remove an entry when it is fixed or decided.

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

From the Grok Build plan (`docs/superpowers/plans/2026-10-01-grok-build.md`)
and its open questions (`.superpowers/sdd/2026-10-01-grok/open-questions.md`).

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
- **Q1, Q2, Q3 and Q5, answered as recommended on 2026-10-01.** If the owner
  reverses one, this changes:
  - Q1 (tool approval documented, not written): writing
    `MCPTool(clax_grok__*)` into `~/.grok/config.toml` needs a
    comment-preserving TOML edit in `clax init` and a matching removal in
    `clax uninit`; the doctor's `mcp` detail and the READMEs then change.
  - Q2 (sandboxed Grok unsupported unless a daemon already runs): supporting
    it needs sandbox detection in the shim and a daemon start outside the
    sandbox; the contract's Known-limitations entry goes.
  - Q3 (Grok Build 1.0.45 and later, warned by doctor only): a different
    minimum changes the doctor's warning and the contract's measurement line;
    enforcing it would add a version check to the shim or hooks.
  - Q5 (no PostToolUse hand-over): see below.
- **A Claude Code fallback for `clax feedback follow`.** A `--once` flag that
  exits after its first line, run by Claude Code as a background command
  whose exit wakes the session, would give Claude Code the same notice path
  as Grok's monitor.
- **A PostToolUse hand-over for Claude Code and Grok together** (Q5). Both
  harnesses can add context to any tool's result; a hand-over there would
  reach the agent on its next tool call of any kind. Add it for both at once,
  as a tier with its own delivery and acknowledgement rules, weighing a
  `clax hook` run on every tool call.
- **`scripts/verify-harnesses.sh` does not cover `grok` yet.** It checks
  `clax init` and `clax uninit` against the real `claude`, `codex` and `pi`
  only.

## Known issues

- **Owner decision: sandbox the subdomain frame** (security pass, O1). A
  subdomain-mode page can navigate the whole window with the viewer's
  activation, to another site or a fresh Clax load (`docs/contract.md`,
  "Known limitations"). Every load now starts with the keyboard trail
  tainted, so what remains is C and `?` live in the fresh load and the
  navigation itself. Giving the subdomain frame `sandbox="allow-scripts
  allow-same-origin allow-forms allow-modals allow-popups
  allow-popups-to-escape-sandbox allow-downloads"` (no top navigation)
  would stop a navigation to another site; it needs every capability and
  page link checked first, and a popup the page opens keeps the shell's
  live keys either way.
- **Owner decision: a keyboard path for consequential actions** (security
  pass, O2). No key clears the tainted trail, so keyboard-only and
  screen-reader viewers need a pointer for Send, Resolve, Reply, the batch
  sends, a page-opened composer's Post, and Allow. A path a page cannot
  coax (for example: the control held focus for `ALLOW_DELAY_MS` with no
  other key in between) would restore keyboard use.
- **Owner decision: rate limits for LAN writes** (security pass, O3). LAN
  viewers can create viewers, threads, comments, sends and `db` documents
  without limit.

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

- **The daemon latency gate covers one viewer on loopback**
  (`scripts/perf-daemon.sh`). It loads the daemon with gallery refreshes,
  gallery loads, `docs:batch` writes, long polls and event streams, and big
  publishes, one at a time. It does not run them together, does not cover
  several viewers, asset uploads, MCP `wait_for_feedback`, the sweepers or
  `clax doctor` (none of which stalled cheap requests when measured by
  hand), and judges no request slower than its probes, such as
  `GET /api/artifacts`. A release build for the gate costs about a minute
  when nothing is cached.
- **The daemon compresses no response** (Svelte port, Task 12). The shell's
  JavaScript, the bridge and its parts, and wrapped pages go out
  uncompressed. On loopback this costs little; on a LAN view it lengthens
  link to first paint and to comment ready. Compress text responses
  (`gzip`, or `br` where accepted) and measure time to usable on a LAN link
  before and after.
- **The bridge says nothing while it waits, or when a part recovers**
  (Svelte port, Task 13 review, M4). Both need protocol additions:
  - While a page's own scripts hold up the parse, comment mode and `use()`
    wait for it with no sign to the viewer. The shell should show a hint
    ("waiting for the page") when the bridge reports it is waiting.
  - After a lazy part failed to load (`clax:degraded`), the shell holds it
    failed for that page until the page's next hello: Comment stays off and
    says so, even once the bridge's own retry could load the part, and
    threads whose anchors went unanswered while it failed stay unplaced. The
    bridge should report a part that loads after failing, and the shell then
    clear the failure and its notice and resolve the anchors again.

## Tests

These tests have failed intermittently under heavy machine load and passed
on rerun; each is parked for a fix:

- `clax-mcp` `open_status`: relies on a 1.5 s wait for the fake browser opener.
- `web/e2e/gesture.spec.ts`, the N14 sandbox closed-shadow-root
  `sendToClaude` case: a 120 s timeout under load. Suspected fix (frame
  load before click) in e5dfa42; confirm under a loaded full gates run.
- `web/shell/src/artifact.test.ts`, "says so when the page of an opened thread
  never greets": races a 50 ms wait against 120 ms sleeps.
- `web/e2e/subpages.spec.ts`, "sandbox: one link inside the frame is one
  history entry": failed about 1 run in 120 under load; passed 10 of 10 alone.
- `clax-hooks` golden `no_daemon_prints_nothing_and_starts_none`:
  timing under load.
- `web/e2e/artifact.spec.ts`, "sandbox: a second viewer's vote 2 s after
  another viewer's vote reloaded it publishes": the reloaded frame did not
  appear within 30 s once in a full gates run; passed 10 of 10 alone.
- `web/e2e/echo-chrome.spec.ts`, "subdomain: the artifact deleted while the
  viewer types in it leaves the keyboard free: Tab reaches the shell's
  controls": took 6.2 min in one gates run; passed 5 of 5 alone.

`scripts/quality_gates.sh` takes a lock per checkout and is read whole before
it runs, so concurrent runs and mid-run edits no longer break it.

