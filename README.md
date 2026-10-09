# Clax

Clax is a local server for HTML artifacts that agents publish. It stores every version of each artifact and serves a gallery and viewer in your browser, where people comment on a page and send comments back to the agent that published it.

## Prerequisites

Rust 1.94 (pinned by `rust-toolchain.toml`), Node 22, and `just`. The tests
also need git 2.44 or later on `PATH`: the git capture tests build fixture
repositories with it, and Clax captures nothing under an older git.

## Install

Install the plugin for your harness; that is all. The plugin downloads the
Clax release it pins on first use (see "Which clax the plugins run" below).

- Claude Code: `/plugin marketplace add empathic/clax`, then
  `/plugin install clax@clax`.
- Codex, from a clone of this repository (nothing needs building):
  `codex plugin marketplace add <clone>`, then `codex plugin add clax@clax`.
- Grok Build, from a clone: `grok plugin install <clone>/plugins/clax-grok --trust`.
- Pi, from a clone: `pi install <clone>/plugins/pi`.

In Claude Code the tools work in the same session once `/plugin install`
reports the plugin active; its `SessionStart` hook, which adds the daemon
URL and any waiting comments to the session's context, runs from the next
session on. In the other harnesses, start a new session afterwards.

In Claude Code's Manual mode, each Clax tool call asks first, `status`
included, until you allow the tools: the plugin does not approve them for
you, outside its own `/clax:` commands. Add
`"mcp__plugin_clax_clax__*"` (every Clax tool), or at least the read-only
ones, to `permissions.allow` in `~/.claude/settings.json` or a project's
`.claude/settings.json`; the Claude Code plugin's README ("Allowing the
tools") has the lines.

The first start of the plugin's MCP server
downloads the pinned release (about 10 MB) into `~/.clax/bin/<version>/`,
checks it against the checksum the plugin carries, and runs it; one download
serves every harness. Hooks never download, so a session's first hooks may
do nothing while the MCP server is still downloading.

### From a clone, with your own build

```
just install
```

It builds the web UI and `clax` (see Prerequisites), installs `clax` into
`$CARGO_HOME/bin` (`~/.cargo/bin` by default; `cargo install --locked --root
"$CARGO_HOME" --path crates/clax-cli`), and runs `clax init`. When your
agents' daemon (the one for `~/.clax`) runs that `clax`, `just install`
stops it with `clax stop`, so the next agent call starts it again from the
new build. Without that, a reinstall at the same version would leave the old
build running, since clients keep a daemon of their own version. A daemon of
any other executable, such as `just watch --shared`'s, is left running and
named. `clax init` writes the Clax plugins built into that binary to
`~/.clax/marketplace/`, registers them with each harness whose CLI is on
your `PATH` (Claude Code, Codex, Pi and Grok Build), and sets the `bin`
setting in `~/.clax/config.toml` to itself, so the plugins run your build
rather than the release they pin. The registration replaces any older one,
including registrations under Clax's previous name. Start a new session in
each harness afterwards. Moving or deleting the clone afterwards changes
nothing.

For Codex, `clax init` also offers, once, to approve the Clax tools that
replace or remove data (`delete` and the `db_*` writes), which Codex
otherwise stops to ask about mid-task: it prints the lines for
`~/.codex/config.toml` and adds them when you say yes (`--yes` for
scripts). The other tools run without asking. `plugins/clax/README.md`
("Tool approval") has the details.

For Grok Build, `clax init` installs the clax-grok plugin (`grok plugin
install <dir> --trust`) whenever `grok` is on your `PATH`, including when
Grok is the only harness installed; `clax uninit` uninstalls it. Grok also
discovers the Claude Code plugin from `~/.claude`; enabled there, that copy
stands down and only clax-grok's tools (`clax_grok__publish` and the rest)
act. Grok asks before each MCP tool call; for headless runs, pass
`--always-approve` or `--allow 'MCPTool(clax_grok__*)'`.

`just uninstall` reverses it: `clax uninit` removes the registrations (and
`~/.clax/marketplace/` once no harness refers to it) and the `bin` setting
when it names the installed `clax`, the agents' daemon is stopped when it
runs that `clax`, then `cargo uninstall clax-cli` removes `~/.cargo/bin/clax`.
It leaves `~/.local/bin/clax` alone, since that one comes from `install.sh`,
and never touches your data in `~/.clax`.

### The `clax` command on PATH, without a clone

```
curl -fsSL https://github.com/empathic/clax/releases/latest/download/install.sh | bash
```

`install.sh` puts the release's `clax` in `~/.local/bin` (or
`$CLAX_INSTALL_DIR`), after checking it against the release's `SHA256SUMS`.
The plugins do not need it. `clax init` then registers that binary's plugins
and points them at it, as `just install` does. GitHub serves release files
only while the repository is public; for a private repository both
`install.sh` and the plugins' download get a 404.

### Which clax the plugins run

Every plugin (Claude Code, Codex and Grok Build through
`scripts/ensure-clax.sh`, Pi through its copy of the same script) runs, in
order:

1. `CLAX_BIN`, when set: it must be a usable `clax`, or that is the error.
2. The `bin` setting in `~/.clax/config.toml` (`$CLAX_HOME/config.toml`),
   one line, `bin = "<absolute path>"`, before any table. `clax bin set
   <path>` (or `clax bin set --this`) writes it, `clax bin clear` removes it,
   and `clax bin` shows what the plugins run and why. `clax init` sets it;
   `clax uninit` clears it.
3. The release the plugin pins, installed in `~/.clax/bin/<version>/clax`
   with its sha256 in `clax.sha256` beside it. The wrapper checks that hash,
   then `--version`, before every run, and downloads the release again when
   either fails. After installing a release it removes older version
   directories but the newest of them, which a daemon started by the
   previous plugin may still be running from.

`PATH` is never consulted. A binary named by `CLAX_BIN` or the `bin` setting
whose version differs from the plugin's runs with a warning. When a plugin
half works, `clax doctor --agent <claude|codex|pi|grok>` checks each layer;
its `binary` check shows which `clax` the plugins run and why. If the MCP
server cannot run `clax` at all, its one tool, `status`, says why, and
`~/.clax/logs/hooks.log` has the details.

## Use

```
clax publish index.html --dir site   # publish a directory; prints the artifact URL (--json gives the ID)
clax open <ID>                       # open an artifact in the browser
clax list                            # list artifacts
clax read <ID> --path style.css      # print a published file (--json gives the read tool's result)
clax asset upload <ID> photo.png     # upload assets; prints each asset URL
clax comments                        # open comment threads on every artifact, newest activity first (--all adds resolved)
clax comments <ID>                   # one artifact's threads
clax comments <ID>#2                 # show thread #2 in full (also: a thread ID)
clax comments reply <ID>#2 "Thanks"  # reply as you (- reads stdin); also resolve, reopen, send
clax comments name "Alex"            # your name, in the CLI and every browser of yours
clax versions <ID>                   # versions: label, publisher, time, threads addressed
clax db get <ID> tasks t1            # the page database: get, list, query, set, update, delete, str-replace, batch
clax status                          # show whether the daemon is running and which agents are working on what
clax doctor                          # check the home directory, daemon, database, and stored files
clax doctor --fix                    # also remove stray temp files and stale rows (never live artifacts' rows)
clax doctor --agent codex            # also check each layer of a harness's plugin: claude, codex, pi or grok
clax stop                            # stop the daemon
clax serve --bind 0.0.0.0            # serve on the LAN (stop a running daemon first)
clax init                            # register the plugins with each harness (clax uninit removes them)
clax bin                             # show which clax the plugins run (clax bin set <path> | --this, clax bin clear)
clax haiku                           # print one of ten haiku about Clax
```

The daemon starts automatically on first use. Data lives in `~/.clax`; set `CLAX_HOME` to use a different directory. The daemon listens on port 7480, or on the `[serve] port` that the home's `config.toml` sets; `CLAX_PORT` overrides the file, and `--port` overrides both. When another program already holds that port, the daemon takes the next free one of the following 20 and its URLs say so; only when all are held, or when you chose the port with `CLAX_PORT` or `--port`, does the plugins' MCP server stop and name a free port to set instead. Clax never stops what holds a port.

Pages that declare `sample` ask Claude with an Anthropic API key on your machine. The daemon reads the key from `ANTHROPIC_API_KEY` when it starts; the `[sample]` table in `config.toml` changes that:

```toml
[sample]
provider = "anthropic"               # or "stub", which echoes the prompt (tests and demos)
api_key_env = "ANTHROPIC_API_KEY"    # the environment variable that holds the key
daily_call_cap = 200                 # optional: at most this many calls per artifact a day
```

Only your own browser spends the key: the page asks you to allow it once each time it loads, and the top bar counts today's calls. A viewer on another machine never can. Without a key, or with an invalid `[sample]` table, sample is off and pages hide the feature; `clax doctor` reports which on its `sample` line. In a checkout, `just demo-room-sample` publishes a room page and a sample page in a scratch daemon, with the stub provider unless `ANTHROPIC_API_KEY` is set.

A new artifact needs a title: `--title`, or else the page's `<title>`. Updates keep the current title unless `--title` is given.

To serve on the LAN, stop a running daemon first, then run `clax serve --bind 0.0.0.0`.

## Use from an agent

Each harness gets the same twenty-four tools (`publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`, `asset_upload`, `status`, `comments_read`, `comments_reply`, `comments_resolve`, `watch`, `wait_for_feedback`, `working`, `ask`, `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`, `db_str_replace`, `db_batch`) and the `clax` skill. Install the plugin for each harness (see Install); plugin details: [plugins/claude-code/README.md](plugins/claude-code/README.md), [plugins/clax/README.md](plugins/clax/README.md), [plugins/pi/README.md](plugins/pi/README.md), [plugins/clax-grok/README.md](plugins/clax-grok/README.md).

The plugins run `CLAX_BIN`, else the `bin` setting, else the release they
pin (see "Which clax the plugins run"). `status` and `clax doctor --agent`
report the binary and its version. `~/.clax/logs/hooks.log` has a line for
every hook run, every MCP start and every install.

Comments wake an idle Codex or Pi session on their own. An idle Claude Code
session wakes in one of two ways. Launched with
`claude --dangerously-load-development-channels plugin:clax@clax` (Claude
Code channels, a research preview: CLI only, claude.ai or Console login,
and on Team and Enterprise an Owner must turn channels on), Clax sends it
a notice through the channel. Otherwise the skill keeps a background
`clax feedback follow --once` running after a publish, and its exit wakes
the session. A notice only points at the comment. The comment itself still
arrives once, through the next clax tool call, the end of the turn, or
`wait_for_feedback`. `status` and `clax doctor --agent claude` show which
path a session uses.

## Documentation

[docs/contract.md](docs/contract.md) is the contract for agents and integrators: every tool's arguments, results and error codes, how sessions are identified per harness, the page contract, the security model, and what is not yet available.

## Upgrading

Pages viewed before this version may still be cached by the browser (older daemons marked supporting HTML pages immutable, and Chrome can keep serving such a copy inside the artifact frame even after "Clear site data"). After upgrading from such a daemon, clear "Cached images and files" for "All time" once, at chrome://settings/clearBrowserData. From this version on, every HTML page is revalidated on each load and the bridge URL names its version, so upgrades never need this.

### From an earlier source install

Earlier versions guessed which `clax` to run: from `PATH`, `~/.local/bin`,
`~/.clax/bin`, or a checkout's `target/`. Now the plugins run `CLAX_BIN`,
else the `bin` setting, else the release they pin, and never look on `PATH`.
In the checkout, run `just install`, which also sets the `bin` setting to
the installed build.
It re-registers every harness from `~/.clax/marketplace/`, which replaces
registrations that pointed at an old or moved checkout, and removes those
under Clax's previous name. Then remove binaries that nothing should run:
check `which -a clax`, and delete an old `~/.local/bin/clax` or
`~/.clax/bin/clax`. The plugins no longer read `CLAX_SOURCE_DIR`; unset it
wherever you set it. Check with `clax doctor --agent <claude|codex|pi|grok>`.

A newer daemon is never replaced by an older `clax`. After installing an
older version, run `clax stop` once. An older daemon is replaced by the
Claude Code and Codex plugins' MCP server and by `clax serve`, not by Pi or
the other CLI commands, so after an upgrade that came from `install.sh`, a
machine that uses only Pi or the CLI keeps the older daemon until `clax
stop`. `just install` stops it itself.

## Developing Clax

Two loops, both on a home of their own, `~/.clax-dev`, whose daemon listens on
port 7481 (its `config.toml` says `[serve] port = 7481`, or the port in
`CLAX_DEV_PORT`, written by whichever of `just watch` and `just dev` runs
first). Your agents' home, `~/.clax`, their daemon on 7480, and the `clax`
they have installed are never touched.

**The daemon and the web UI: `just watch`.** An auto-reloading server at
http://localhost:7481, which can be left running. A Rust change rebuilds and
restarts the daemon; a web change rebuilds `web/dist` (reload the browser).
Extra arguments go to `serve` (`just watch --bind 0.0.0.0`). Ctrl-C stops
everything. `CLAX_DEV_PORT=<port> just watch` uses another port, and a
`CLAX_HOME` you set is used as the home. `just watch --shared` serves the
agents' home, `~/.clax`, on 7480 instead: stop their daemon first with
`clax stop`, and expect an agent to start its own daemon there while a Rust
change rebuilds. `just dev` with no harness, or with an option first
(`just dev --shared`), runs `just watch`. `just serve`, `just stop` and
`just doctor` run the checkout's build on `~/.clax-dev` (`just serve` on
7481). `just watch` needs `cargo-watch` (`cargo install cargo-watch`).

**An agent on your working tree: `just dev claude|codex|grok|pi`.** It builds
`clax`, copies it into a temporary directory put first on `PATH` (removed
when the session ends), and starts the harness on `~/.clax-dev`:

```
just dev claude
just dev codex
just dev grok
just dev pi
just dev claude --resume  # extra arguments go to the harness
```

- Claude Code loads `plugins/claude-code` straight from the checkout
  (`--plugin-dir`), and an installed `clax@clax` is disabled for that
  session.
- Pi loads `plugins/pi/src/clax.ts` and its skill from the checkout. It runs
  with `-ne` so an installed Clax package does not load twice, which turns
  off your other Pi extensions for that session too.
- Codex cannot load a plugin from a directory, so `just dev codex` runs your
  installed Clax plugin, with your own `~/.codex` as it is, on the fresh
  build and `~/.clax-dev`. To try plugin, skill or hook changes in Codex, run
  `just install`.
- Grok works like Codex: `just dev grok` runs your installed clax-grok
  plugin, with your own `~/.grok`, on the fresh build and `~/.clax-dev`. To
  try plugin, skill or hook changes in Grok, run `just install`.

Edit the plugin, skill or hooks, then start a new `just dev` session to pick
them up. A dev-home daemon left by an earlier `just dev`, whose temporary
binary is gone, is stopped at the next `just dev` or `just watch`, which says
so; a `just watch` daemon is left alone, and `~/.clax` and port 7480 are never
touched. `just install` puts the working tree in front of your everyday agents
(their daemon restarts on the new build at their next call), and `just
uninstall` takes it away again.

- `just help` (or bare `just`) lists every recipe with a description.
- `just check` formats the Rust code, then runs every quality gate.
- `just ci` runs the same gates CI runs, without formatting.
- `just perf` runs the perf gates' full versions (the gates run their quick ones).

Each crate's integration tests are one test binary, `tests/integration.rs`, with
a module per file in `tests/`; the few that must not share a process with
the rest have a second binary (`clax-cli/tests/descriptors.rs`,
`clax-server/tests/web_dist.rs`). On macOS every new test
binary is assessed on its first run, after each rebuild. A new file in `tests/`
runs once a binary's root declares it (`mod name;`); a test checks that each
one is. To run
one file's tests: `cargo nextest run -p clax-server -E 'test(/^api_batch::/)'`,
or `cargo test -p clax-server --test integration api_batch::`.

`scripts/quality_gates.sh` runs every check CI runs: the justfile, plugin wrapper (`scripts/test-ensure-clax.sh`, `just wrapper-test`), release script, release installer and dev script tests (`scripts/test-release.sh`, `scripts/test-install.sh` (`just install-test`), `scripts/test-dev.sh`), and plugin structure tests (`scripts/test-plugins.sh`, which also checks that the four skill copies and `docs/contract.md` share the page contract word for word, that the skill copies share the comment loop word for word, that the plugins wire their Stop and prompt hooks, that the workspace, the plugin manifests, the Pi package and the plugins' wrapper's `CLAX_VERSION` carry one version (`scripts/check-version.sh`), that the four plugins carry the same wrapper and pin the newest `v*` release tag, that the Rust and Pi tool descriptions match `plugins/pi/test/fixtures/contract.json`, and, through `scripts/sync-skill-tools.py --check`, that each skill's generated tool block and the tool lists in `docs/contract.md` and the READMEs name exactly that fixture's tools), the web lint (`oxlint`, configured in `web/.oxlintrc.json`), web typecheck and unit tests, the web build (before the Rust tests and the release build, which serve or embed it), `cargo fmt`, clippy with `-D warnings` over every target and again over the libraries and binaries alone (without the test-only features), the Rust tests (`cargo nextest`, or `cargo test` with a warning where nextest is not installed: `cargo install --locked cargo-nextest`), the comment-loop smoke (`scripts/smoke-comment-loop.sh`: a scripted Claude Code session, no model, through tiers 1, 2, 4 and 5 with a fake `codex`), the Pi extension's typecheck and tests, the three perf gates in their quick mode, and the Playwright end-to-end tests. `just web-test` runs the web lint, typecheck, and unit tests.

The web UI is built first (`scripts/build-web.sh`, which keeps the modification times of output it did not change, so an unchanged web UI does not rebuild the release binary that embeds it) and its unit tests run next, alone, since some are timing-sensitive. The other gates then run in concurrent lanes (web e2e, the Rust tests, clippy and `cargo fmt`, web lint and typecheck, the release build, Pi, the scripts), the longest first, each printing a line as it finishes; a failed gate prints its output. The Rust tests are built before clippy runs, so they start as soon as they can. `npm ci` runs in `web/` and `plugins/pi/` only when `node_modules` was not installed from the current `package-lock.json` by the same Node and npm (a hash in `node_modules/.clax-ci-stamp`; `CLAX_NPM_CI=always` ignores it). One debug `clax` is built for the whole run and given to the tests that cannot name it and to the browser tests' daemons as `CLAX_TEST_BIN` (`just test` does the same for the Rust tests), and one release `clax` to the perf gates as `CLAX_PERF_BIN`. The perf gates then run alone, one at a time. The run ends with each gate's time and the total; a slow gate is reported, never failed. On a warm build cache and an otherwise idle machine, the whole run takes under two minutes.

The perf gates, each with the same budgets and the same scaling by the run's own idle latency in its quick and full versions; `just perf` runs the full versions:

- daemon latency (`scripts/perf-daemon.sh`): a release `clax` on a scratch home seeded with 300 artifacts, 2,400 threads and 7,200 comments by one viewer; while a gallery tab refreshes, ten galleries load, 12 MB `docs:batch` writes, long polls and event streams, and 16 MB publishes run in turn, `/healthz`, `/a/<id>`, `/c/<id>/v/1/` and `/api/artifacts/<id>` must keep a p95 of 50 ms and a max of 250 ms, and the gallery's list and attention requests alone, each timed against a fixed SQLite calibration read that the daemon runs on its own store workers, interleaved with it, must stay within about 1.4 times their measured ratios to it, so either query getting twice as slow fails on a fast machine, a slow one or a loaded one; medians over three rounds of 3 s windows per load (0.5 s in the quick mode), limits scaled by the run's own idle latency, and under the ten galleries to at least twice the galleries' own mean request latency, budgets in `scripts/perf-daemon-budget.json`;
- realtime clients (`scripts/perf-clients.sh`): 1,000 `/api/stream` clients on one release daemon, judged on delivery latency, cheap requests under load, memory per client, idle CPU and a client that never reads; a 16 s idle and a 15 s load window (3 s and 9 s in the quick mode: the load makes at least 40 writes, so one write held up by its commit cannot set the delivery p95 alone), budgets in `scripts/perf-clients-budget.json`;
- time to usable (`cd web && npm run perf`): link to first paint and to comment mode in a fresh tab, per frame mode, against `web/perf/budget.json`; the quick mode (`CLAX_PERF_QUICK=1`) takes 5 samples per mode instead of 9.

One run of the gates at a time per checkout: a run holds
`quality-gates.lock` in the checkout's git directory, a second run in the
same checkout waits for it, and a lock whose process has gone is taken over.
Separate worktrees still run in parallel.

`scripts/verify-harnesses.sh` (manual) checks `clax init` and `clax uninit`
against the real `claude`, `codex` and `pi` CLIs, entirely inside a scratch
root it deletes on exit, and prints a PASS/FAIL table. It tests `CLAX_BIN`,
or a fresh `cargo build`, never your installed `clax`; it refuses to run when
a scratch directory would fall inside your home, never reads or copies an
auth file, and runs Clax on a scratch port. The Pi session check, which needs
a model, runs only with `VERIFY_PI_SESSION=1` and a provider key you export.

## Releasing

A release is what the plugins download, and what `install.sh` installs.
Only a person cuts one. Its downloads work only while the repository is
public.

```
scripts/bump-version.sh 0.4.0     # every version, and the skills' tool blocks
just ci
git commit -am "Release 0.4.0"
git tag -s v0.4.0 -m "Clax 0.4.0"
git push origin v0.4.0            # the tag alone: main moves once the release is pinned
# wait for the Release workflow to publish v0.4.0, then:
scripts/pin-release.sh v0.4.0     # PINNED_VERSION and the four SHA256 values, in every wrapper copy
just ci
git commit -am "Pin the plugins to Clax 0.4.0"
git push origin main
```

A plugin pins the release its wrapper names (`PINNED_VERSION` and one
`SHA256_*` per target in `scripts/ensure-clax.sh`; `scripts/pin-release.sh`
writes them, from the release's `SHA256SUMS`, into that file and into every
plugin's copy, Pi's included). A release cannot pin itself, since its
checksums exist only once it is built, so the tagged commit pins the
previous release and the pin follows in the next commit. `scripts/test-plugins.sh`
(and so `just ci` and CI, which fetches the tags) fails while the pin is
older than the newest `v*` tag or newer than every tag; with no tag at all,
nothing may be pinned. Harnesses update an installed plugin only when its
version changes, so main gets the new version and its pin in one push:
push the tag first, and main after pinning. The binary a release builds
carries plugins that pin the previous release; `clax init` points them at
that binary through the `bin` setting.

The tag runs `.github/workflows/release.yml`. It checks that the tag matches
every version, then builds macOS arm64 and x86_64 and Linux x86_64 and arm64
(static musl) binaries on native runners, each with the web UI embedded. It
smoke-tests each binary and packs `clax-<version>-<target>.tar.gz`. It
installs one with `install.sh` from a local copy of the release, and
another through the plugins' wrapper pinned to that copy, and publishes the archives, `install.sh` and `SHA256SUMS`. Running the workflow
by hand (Actions, Release, Run workflow), or a pull request that touches the
release path, does everything except publish, and keeps the result as the
`release-dist` artifact.

## Security model

Writes, and reads of the session list (working directories, process IDs, harness session IDs), need the token in `~/.clax/daemon.json` (mode 0600, served only to localhost browsers), so LAN viewers can read artifacts and comment on them but not publish or change them. Every `/api` route answers only to a `Host` of `localhost`, `127.0.0.1`, `[::1]`, or the exact IP and port the connection arrived on (403 `forbidden_host` otherwise), which defeats DNS rebinding. The comment routes need no token but refuse requests from another origin, including published pages. The viewer cookie never leaves the daemon; viewers are named by a public ID, comment threads carry no session IDs, and the daemon's `codex` path is served only with the token. Published content is isolated on `<id>.localhost` origins or sandboxed. Content and asset URLs are fetchable by anyone who knows the unguessable ID.

Design: [docs/superpowers/specs/2026-09-28-clax-design.md](docs/superpowers/specs/2026-09-28-clax-design.md)

Runtime capabilities arrive in a later phase.
