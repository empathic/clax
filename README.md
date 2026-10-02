# Clax

Clax is a local server for HTML artifacts that agents publish. It stores every version of each artifact and serves a gallery and viewer in your browser, where people comment on a page and send comments back to the agent that published it.

## Prerequisites

Rust 1.94 (pinned by `rust-toolchain.toml`), Node 22, and `just`.

## Install

From a clone of this repository (see Prerequisites):

```
just install
```

It builds the web UI and `clax`, installs `clax` into `$CARGO_HOME/bin`
(`~/.cargo/bin` by default; `cargo install --locked --root "$CARGO_HOME"
--path crates/clax-cli`), and runs `clax init`. When your agents' daemon (the
one for `~/.clax`) runs that `clax`, `just install` stops it with `clax
stop`, so the next agent call starts it again from the new build. Without
that, a reinstall at the same version would leave the old build running,
since clients keep a daemon of their own version. A daemon of any other
executable, such as `just watch --shared`'s, is left running and named.
`clax init` writes the Clax plugins built into that binary to
`~/.clax/marketplace/` and registers them with each harness whose CLI is on
your `PATH`: Claude Code, Codex, Pi and Grok Build. The registration replaces
any older one, including registrations under Clax's previous name. Start a
new session in each harness afterwards. The plugins run the `clax` on the `PATH` the
harness starts with, so `~/.cargo/bin` must be on it; `clax init` warns
when the first `clax` on your `PATH` is another one. Nothing is downloaded,
and moving or deleting the clone afterwards changes nothing.

For Grok Build, `clax init` installs the clax-grok plugin (`grok plugin
install <dir> --trust`) whenever `grok` is on your `PATH`, including when
Grok is the only harness installed; `clax uninit` uninstalls it. Grok also
discovers the Claude Code plugin from `~/.claude`; enabled there, that copy
stands down and only clax-grok's tools (`clax_grok__publish` and the rest)
act. Grok asks before each MCP tool call; for headless runs, pass
`--always-approve` or `--allow 'MCPTool(clax_grok__*)'`.

`just uninstall` reverses it: `clax uninit` removes the registrations (and
`~/.clax/marketplace/` once no harness refers to it), the agents' daemon is
stopped when it runs the installed `clax`, then `cargo uninstall clax-cli`
removes `~/.cargo/bin/clax`. It leaves `~/.local/bin/clax` alone,
since that one comes from `install.sh`, and never touches your data in
`~/.clax`.

Without a clone, use the release installer:

```
curl -fsSL https://github.com/empathic/clax/releases/latest/download/install.sh | bash
clax init
```

`install.sh` puts the release's `clax` in `~/.local/bin` (or
`$CLAX_INSTALL_DIR`), after checking it against the release's `SHA256SUMS`.
It works only once the repository is public: it is private today, and GitHub
serves a private repository's release files only to authenticated requests,
so until then `install.sh` gets a 404. When to make it public is the
repository owner's decision.

When a plugin half works, `clax doctor --agent <claude|codex|pi|grok>` checks each
layer. Its `binary` check shows which `clax` the plugins run and every `clax`
on `PATH`. If the MCP server cannot find `clax` at all, its one tool,
`status`, says why, and `~/.clax/logs/hooks.log` has the details.

## Use

```
clax publish index.html --dir site   # publish a directory; prints the artifact URL (--json gives the ID)
clax open <ID>                       # open an artifact in the browser
clax list                            # list artifacts
clax read <ID> --path style.css      # print a published file (--json gives the read tool's result)
clax asset upload <ID> photo.png     # upload assets; prints each asset URL
clax status                          # show whether the daemon is running
clax doctor                          # check the home directory, daemon, database, and stored files
clax doctor --fix                    # also remove stray temp files and stale rows (never live artifacts' rows)
clax doctor --agent codex            # also check each layer of a harness's plugin: claude, codex, pi or grok
clax stop                            # stop the daemon
clax serve --bind 0.0.0.0            # serve on the LAN (stop a running daemon first)
clax init                            # register the plugins with each harness (clax uninit removes them)
clax haiku                           # print one of ten haiku about Clax
```

The daemon starts automatically on first use. Data lives in `~/.clax`; set `CLAX_HOME` to use a different directory. The daemon listens on port 7480, or on the `[serve] port` that the home's `config.toml` sets (`--port` overrides both).

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

Each harness gets the same twenty-two tools (`publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`, `asset_upload`, `status`, `comments_read`, `comments_reply`, `comments_resolve`, `watch`, `wait_for_feedback`, `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`, `db_str_replace`, `db_batch`) and the `clax` skill. `clax init` registers the plugin with each harness (see Install); plugin details: [plugins/claude-code/README.md](plugins/claude-code/README.md), [plugins/clax/README.md](plugins/clax/README.md), [plugins/pi/README.md](plugins/pi/README.md), [plugins/clax-grok/README.md](plugins/clax-grok/README.md).

The plugins run `clax` from `PATH` (or `CLAX_BIN`, for scripts) through
`scripts/ensure-clax.sh`, which never downloads or builds anything. A `clax`
whose version differs from the plugin's runs with a warning, and `status` and
`clax doctor --agent` report the difference; `just install` (or `clax init`)
brings them back in step. The Pi extension runs `CLAX_BIN`, else the `clax`
on `PATH`, the same way. `~/.clax/logs/hooks.log` has a line for every hook
run and every MCP start.

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
`~/.clax/bin`, or a checkout's `target/`. Now the plugins run the `clax` on
`PATH`, and `clax init` registers them. In the checkout, run `just install`.
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

`scripts/quality_gates.sh` runs every check CI runs: the justfile, plugin wrapper (`scripts/test-ensure-clax.sh`, `just wrapper-test`), release script, release installer and dev script tests (`scripts/test-release.sh`, `scripts/test-install.sh` (`just install-test`), `scripts/test-dev.sh`), and plugin structure tests (`scripts/test-plugins.sh`, which also checks that the four skill copies and `docs/contract.md` share the page contract word for word, that the skill copies share the comment loop word for word, that the plugins wire their Stop and prompt hooks, that the workspace, the plugin manifests, the Pi package and the plugins' wrapper's `CLAX_VERSION` carry one version (`scripts/check-version.sh`), that the Rust and Pi tool descriptions match `plugins/pi/test/fixtures/contract.json`, and, through `scripts/sync-skill-tools.py --check`, that each skill's generated tool block and the tool lists in `docs/contract.md` and the READMEs name exactly that fixture's tools), the web lint (`oxlint`, configured in `web/.oxlintrc.json`), web typecheck and unit tests, the web build (before the Rust gates, which serve or embed it), `cargo fmt`, clippy with `-D warnings`, `cargo check` without test features, `cargo test`, the comment-loop smoke (`scripts/smoke-comment-loop.sh`: a scripted Claude Code session, no model, through tiers 1, 2, 4 and 5 with a fake `codex`), the Pi extension's typecheck and tests, and the Playwright end-to-end tests. `just web-test` runs the web lint, typecheck, and unit tests.

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

Releases are for people without a checkout. Only a person cuts one, and
`install.sh` can fetch it only once the repository is public.

```
scripts/bump-version.sh 0.4.0   # every version, and the skills' tool blocks
just ci
git commit -am "Release 0.4.0"
git tag -s v0.4.0 -m "Clax 0.4.0"
git push origin main v0.4.0
```

The tag runs `.github/workflows/release.yml`. It checks that the tag matches
every version, then builds macOS arm64 and x86_64 and Linux x86_64 and arm64
(static musl) binaries on native runners, each with the web UI embedded. It
smoke-tests each binary and packs `clax-<version>-<target>.tar.gz`. It
installs one with `install.sh` from a local copy of the release, and
publishes the archives, `install.sh` and `SHA256SUMS`. Running the workflow
by hand (Actions, Release, Run workflow), or a pull request that touches the
release path, does everything except publish, and keeps the result as the
`release-dist` artifact.

## Security model

Writes, and reads of the session list (working directories, process IDs, harness session IDs), need the token in `~/.clax/daemon.json` (mode 0600, served only to localhost browsers), so LAN viewers can read artifacts and comment on them but not publish or change them. Every `/api` route answers only to a `Host` of `localhost`, `127.0.0.1`, `[::1]`, or the exact IP and port the connection arrived on (403 `forbidden_host` otherwise), which defeats DNS rebinding. The comment routes need no token but refuse requests from another origin, including published pages. The viewer cookie never leaves the daemon; viewers are named by a public ID, comment threads carry no session IDs, and the daemon's `codex` path is served only with the token. Published content is isolated on `<id>.localhost` origins or sandboxed. Content and asset URLs are fetchable by anyone who knows the unguessable ID.

Design: [docs/superpowers/specs/2026-09-28-clax-design.md](docs/superpowers/specs/2026-09-28-clax-design.md)

Runtime capabilities arrive in a later phase.
