# Using and developing Clax

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
- Codex: `codex plugin marketplace add empathic/clax`, then
  `codex plugin add clax@clax` (a clone works in place of
  `empathic/clax`).
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
clax connect <invite URL>            # connect this machine to someone else's Clax as a person (--status, --off)
clax people                          # (the owner) list people, their rights, machines and agents, and the People link
clax agents                          # list your agents; clax agents rename <agent> <name> renames one
clax approvals                       # list whose content reaches your agents, and what waits for you
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

To serve the internet (hosted mode), put the daemon behind a proxy that terminates TLS and keeps the `Host` header, and name its public origin in `config.toml`:

```toml
[hosted]
public_origin = "https://clax.example"   # https, or http on localhost for development
setup_secret_env = "CLAX_SETUP_SECRET"   # the default: the variable the setup secret is read from
behind_cloudflare = false                # true when Cloudflare is in front: clients are counted by CF-Connecting-IP
```

Start the daemon with the secret in that variable and open `https://clax.example/setup` to make the first person, the owner; without a secret, setup stays closed. From then on people sign in through the browser: the owner invites a person on People (a link, `/join/<code>`, good for a day), a signed-in person makes a one-use, ten-minute sign-in link for another device and agent tokens for their remote agents, and the owner shares a page by link, where anyone may read it and, once they give a name, comment as a guest. Nothing is served as "this machine" in hosted mode (the owner's browser holds a token of its session's, never the daemon token), a person's rights limit their browser as they limit their agents, a page's content is served only at its version's keyed address, setup, sign-in codes and guest names are rate-limited per client address, sign-ins and sign-outs are in the audit journal, and a bad `[hosted]` table stops the daemon from starting.

### People

Other people can connect to your Clax, each with their own machines and
agents. Use a trusted LAN: Clax traffic is not encrypted, so anyone on the
network can read pages and comments as they travel, and a sniffed browser
cookie acts as its person in the browser.

- **Inviting.** Serve on the LAN, open People (`/people`) in your browser
  and choose **Invite a person**. Give them the command it shows, `clax
  connect http://<your LAN address>:<port>/i/<code>`: it works once, within
  24 hours. They run it on their machine, with Clax installed. An invite
  can also add another machine of yours.
- **Asking.** Someone can instead run `clax connect http://<IP>:<port>`.
  It shows a code and waits up to 10 minutes; a connection request comes
  to your inbox, and you approve it there with the code their machine
  shows, a name and the rights to give them (Reader, Collaborator or
  Full).
- **After connecting.** `clax connect` opens their browser signed in as
  them and asks whether your comments and pages may reach their agents.
  Their agents then publish, reply, resolve and write page data with the
  rights you gave, each on a key that never leaves their machine. `clax
  connect --status` checks the connection and `clax connect --off`
  disconnects it.
- **Agents.** Every agent has a name (from its repository and branch),
  shown wherever it appears, with its person beside it when it is someone
  else's. `@<name>` in a comment sends the thread to that agent. People
  shows each person's agents. An agent's person, or you, can rename it.
  Its person can narrow what it may do. You can raise it anywhere, and the
  creator of an artifact can raise it on that artifact.
- **Approvals.** What another person wrote is withheld from your agents
  until you approve them: for one thread, one page or everywhere. When a
  thread is sent to your agent, or your agent asks, an approval card comes
  to your inbox and shows above the thread. The card offers "Allow this
  thread", "Always allow … on this page", "Always allow … everywhere" or
  "Decline", and the thread reads "waiting for your approval" until you
  decide. People lists who reaches your agents and lets you revoke an
  approval. Approving happens only in a browser: `clax approvals` lists,
  and prints the People link.
- **Viewers who are not connected.** Comments from LAN viewers who are not
  connected people reach your agents without asking, as before. The
  switch in People turns that off.
- **Revoking.** Revoking a person or one of their machines in People ends
  their agents' sessions at once. Their browser becomes a plain viewer's,
  and what they wrote stays theirs.

### Publishing to a public Clax

Your Clax can publish artifacts to a hosted one (a daemon in hosted mode, or
the Cloudflare Worker), where they get a public link; nothing changes for
your local Clax or its plugins until you add one.

```
clax remote add https://clax.example   # sign this Clax in there; approve it in the browser
clax remote list                       # which public Clax instances it is signed in to, and as whom
clax push <ID>                         # publish an artifact there, or push its new versions
clax push <ID> --to clax.example       # name the public Clax when there are several
clax push <ID> --follow                # also let agents push its new versions without asking
clax push <ID> --fork                  # the public copy moved: push onto a fork of it
clax push <ID> --new                   # or publish it again as a new public artifact
clax remote comments <ID> download-only  # keep threads started here local (two-way is the default)
clax remote unlink <ID>                # end an artifact's link; its threads stay here, read-only
clax remote remove clax.example        # forget it, revoking its token there
```

`clax remote add` asks the public Clax for a sign-in naming this machine,
prints a code and opens its approval page. Signed in there as the person to
publish as, check the code and approve: this Clax receives an agent token of
yours, which your People page there lists (named `Clax on <host name>`) and
revokes. Codes last ten minutes and work once.

A push publishes every version the public copy has not had, in order, with
its label, note and files. The public versions are yours there: each names
the local agent that made it, and the ones you published by hand read as
yours by hand. An asset a pushed page loads by the address Clax gave it
(`/_blob/<ID>`) is uploaded to the public copy and the page's references
rewritten to its address there; other assets stay on your machine. A push whose answer was lost can be sent
again: the public Clax answers the version it already made. When an
automatic push (`--follow`) is refused, your inbox says why.
If someone published on the public copy since your last push, the push is
refused and nothing is overwritten: push onto a fork of it, or as a new
artifact.

Once pushed, the artifact's comment threads are linked both ways: a comment
on the public copy shows up on your local page tagged with where it came
from and who wrote it there, reaches your agents as a local comment does
(`wait_for_feedback`, channel notifications), and their replies and resolves
go back to the public thread, once each. Threads you start locally go up
too, as you, unless the link is download only (a thread started while it
is download only stays local, even once it is two-way again). While the
public Clax cannot be reached, what you write waits, marked "Sending…",
then "Not sent" with Retry after five minutes; nothing local waits on it.

On an artifact's page, Publish to `<host>` does the same in the browser
(and Push v`<N>` once it is public, beside a link to its public copy). An
agent publishes with the `publish_public` tool, which asks you first: the
question waits in your inbox beside the artifact, and only your answer in
the browser or the extension counts. Choose Publish and push new versions
to let that artifact's later versions go out without asking again.

## Use from an agent

Each harness gets the same thirty tools (`publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`, `fork`, `asset_upload`, `status`, `comments_read`, `comments_reply`, `comments_resolve`, `comments_handoff`, `watch`, `wait_for_feedback`, `working`, `draft`, `git_remote`, `ask`, `where_to_ask`, `publish_public`, `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`, `db_str_replace`, `db_batch`) and the `clax` skill. Install the plugin for each harness (see Install); plugin details: [plugins/claude-code/README.md](../plugins/claude-code/README.md), [plugins/clax/README.md](../plugins/clax/README.md), [plugins/pi/README.md](../plugins/pi/README.md), [plugins/clax-grok/README.md](../plugins/clax-grok/README.md).

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

[docs/contract.md](contract.md) is the contract for agents and integrators: every tool's arguments, results and error codes, how sessions are identified per harness, the page contract, the security model, and what is not yet available.

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

Checks and builds run with Bazel when Bazelisk is installed and with Cargo
and pnpm otherwise; `docs/bazel.md` has which recipe runs what, the
`-native` twins, `just bazel-sync` and `just bazel-gc`.

The Rust workspace is layered, each crate depending only on the crates
above it: `clax-core` (shared types and pure rules), then `clax-store` (the
SQLite store), `clax-sample`, `clax-mcp` and `clax-hooks`; `clax-realtime`
(in-memory delivery); `clax-http` (what every route shares); `clax-server`
(the routes and `serve()`); `clax-commands` (every subcommand);
`clax-embedded` (the files the binary carries); and `clax-cli`, the binary
that wires them together. Test support is `clax-testkit` (the in-process
test daemon, for integration tests only) and `clax-fake-exe`. Beside
`clax-core` sit `clax-rt` (the async runtime seam: tokio natively, a
Worker's event loop on wasm32) and `clax-git` (git objects, packs and smart
HTTP, without I/O), with `clax-content-git` (artifact content kept in git)
over both; the hosted Worker, `crates/clax-worker`, is a workspace of its
own over the daemon crates. `scripts/check-crate-layers.py`, a quality
gate, holds the rule crate by crate (its `ALLOWED` map) and fails a
dependency out of order.

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

**A second person: `scripts/dev-second-person.sh <invite>`.** To try People
(`/people`) and the other multi-person surfaces alone, run the dev daemon on
the LAN (`just watch --bind 0.0.0.0`), open People as the owner
(http://localhost:7481/people), choose **Invite a person**, and pass the
command's invite URL to the script. It connects a scratch Clax home (never
`~/.clax` or the dev home) as that person's machine, prints the browser link
that makes a browser that person (open it at the LAN address), and
registers one agent of theirs. `scripts/dev-second-person.sh --off <home>`
disconnects it and removes the home.

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
- `just perf` runs the perf benchmarks. Performance is measured on demand
  and gates nothing: no check runs them, and they report a measure over its
  budget without failing.

Each crate's integration tests are one test binary, `tests/integration.rs`, with
a module per file in `tests/`; the few that must not share a process with
the rest have a second binary (`clax-cli/tests/descriptors.rs`). On macOS every new test
binary is assessed on its first run, after each rebuild. A new file in `tests/`
runs once a binary's root declares it (`mod name;`); a test checks that each
one is. To run
one file's tests: `cargo nextest run -p clax-server -E 'test(/^api_batch::/)'`,
or `cargo test -p clax-server --test integration api_batch::`.

`scripts/quality_gates.sh` runs every check CI runs: the justfile, plugin wrapper (`scripts/test-ensure-clax.sh`, `just wrapper-test`), release script, release installer and dev script tests (`scripts/test-release.sh`, `scripts/test-install.sh` (`just install-test`), `scripts/test-dev.sh`), the Clax app's build script test (`scripts/test-build-app-clax.sh`, a dry run with no build), and plugin structure tests (`scripts/test-plugins.sh`, which also checks that the four skill copies and `docs/contract.md` share the page contract word for word, that the skill copies share the comment loop word for word, that the plugins wire their Stop and prompt hooks, that the workspace, the plugin manifests, the Pi package and the plugins' wrapper's `CLAX_VERSION` carry one version (`scripts/check-version.sh`), that the four plugins carry the same wrapper and pin the newest `v*` release tag, that the Rust and Pi tool descriptions match `plugins/pi/test/fixtures/contract.json`, and, through `scripts/sync-skill-tools.py --check`, that each skill's generated tool block and the tool lists in `docs/contract.md` and the READMEs name exactly that fixture's tools), the web lint (`oxlint`, configured in `web/.oxlintrc.json`), web typecheck and unit tests, the web build (before the Rust tests and the release build, which serve or embed it), `cargo fmt`, clippy with `-D warnings` over every target, the Rust tests (`cargo nextest`, or `cargo test` with a warning where nextest is not installed: `cargo install --locked cargo-nextest`), the comment-loop smoke (`scripts/smoke-comment-loop.sh`: a scripted Claude Code session, no model, through tiers 1, 2, 4 and 5 with a fake `codex`), the Pi extension's typecheck and tests, the Worker's checks (`crates/clax-worker`, its own workspace: `cargo fmt`, its tests, among them the SQLite VFS's, and clippy, natively, then `test-wasm.mjs`, which builds the wasm Worker, writes through it under `wrangler dev --local` on free ports with fresh state under `crates/clax-worker/target/wasm-test/`, restarts wrangler and checks the data and `PRAGMA integrity_check`; and, after the web e2e tests, `e2e.mjs`, which runs the Clax server in its Durable Object under `wrangler dev --local` with the web UI from `web/dist` and checks in two browsers that a published artifact shows in the gallery and opens, that a comment reaches the other browser live over the stream's WebSocket, also after the server is rebuilt as after hibernation, and that everything is there after a restart), the Clax app's gate (`apps/clax_app/tool/check.sh`, skipped where `flutter` is not installed; see "The Clax app" below), and the Playwright end-to-end tests of the web UI. `just web-test` runs the web lint, typecheck, and unit tests.

The web UI is built first (`scripts/build-web.sh`, which keeps the modification times of output it did not change, so an unchanged web UI does not rebuild the release binary that embeds it) and its unit tests run next, alone, since some are timing-sensitive. The other gates then run in concurrent lanes (web e2e then the Worker's e2e, the Rust tests, clippy and `cargo fmt`, web lint and typecheck, the release build, Pi, the scripts, the Worker, the wasm32 check, the Clax app), the longest first, each printing a line as it finishes; a failed gate prints its output. The Rust tests are built before clippy runs, so they start as soon as they can. `pnpm install --frozen-lockfile` runs in `web/` and `plugins/pi/` only when `node_modules` was not installed from the current `pnpm-lock.yaml` by the same Node and pnpm (a hash in `node_modules/.clax-install-stamp`; `CLAX_PNPM_INSTALL=always` ignores it), and `npm ci` in `crates/clax-worker/`, the one npm project, only when its `node_modules` was not installed from its `package-lock.json` by the same Node and npm (`node_modules/.clax-ci-stamp`; `CLAX_NPM_CI=always` ignores it). One debug `clax` is built for the whole run and given to the tests that cannot name it and to the browser tests' daemons as `CLAX_TEST_BIN` (`just test` does the same for the Rust tests), and one release `clax` to the perf gates as `CLAX_PERF_BIN`. The perf gates then run alone, one at a time. The run ends with each gate's time and the total; a slow gate is reported, never failed. On a warm build cache and an otherwise idle machine, the whole run takes under two minutes.

The perf benchmarks, run on demand by `just perf` and by no check: each prints its measures beside its budgets, scaled by the run's own idle latency, and reports a measure over budget without failing:

- daemon latency (`scripts/perf-daemon.sh`): a release `clax` on a scratch home seeded with 300 artifacts, 2,400 threads and 7,200 comments by one viewer; while a gallery tab refreshes, ten galleries load, 12 MB `docs:batch` writes, long polls and event streams, and 16 MB publishes run in turn, `/healthz`, `/a/<id>`, `/c/<id>/v/1/` and `/api/artifacts/<id>` must keep a p95 of 50 ms and a max of 250 ms, and the gallery's list and attention requests alone, each timed against a fixed SQLite calibration read that the daemon runs on its own store workers, interleaved with it, must stay within about 1.4 times their measured ratios to it, so either query getting twice as slow fails on a fast machine, a slow one or a loaded one; medians over three rounds of 3 s windows per load (0.5 s in the quick mode), limits scaled by the run's own idle latency, and under the ten galleries to at least twice the galleries' own mean request latency; with two people connected by `clax connect` (6,000 comments by four authors, 50 approvals each), a person's agent's threads read and feedback poll (half the rows withheld) within 2 ms and 1 ms of the single-owner baseline that `scripts/perf-vis-baseline.sh <commit>` measures, the gateway's median signature verification within 150 µs, and a signed client polling beside every load; budgets in `scripts/perf-daemon-budget.json`;
- realtime clients (`scripts/perf-clients.sh`): 1,000 `/api/stream` clients on one release daemon, judged on delivery latency, cheap requests under load, memory per client, idle CPU and a client that never reads; a 16 s idle and a 15 s load window (3 s and 9 s in the quick mode: the load makes at least 40 writes, so one write held up by its commit cannot set the delivery p95 alone), budgets in `scripts/perf-clients-budget.json`;
- time to usable (`cd web && pnpm run perf`): link to first paint and to comment mode in a fresh tab, and link to the shell's first `ready` to the Clax app's host (`/a/<id>?app=1`, the end of the app's say → usable time), per frame mode, against `web/perf/budget.json`; the quick mode (`CLAX_PERF_QUICK=1`) takes 5 samples per mode instead of 9.

The numbers are kept in a perf log, one JSON line per run: the commit and
date, the runner, its CPU count and load average, and every measure (the
benchmarks above and the bundle bytes) beside its budget. Each benchmark
writes its measures to `CLAX_PERF_JSON` when it is set, and
`scripts/perf-log.sh record` makes the line. `just perf` prints it and
appends it to `target/perf-log.jsonl`, and never pushes. CI's `perf-log`
job runs the benchmarks after every green push to main and appends its line
to `perf/log.jsonl` on the orphan branch `perf-log`; a number never fails
it. `scripts/perf-log.sh show` fetches that branch and prints each
measure's trend over the last runs (`--runs N`, default 10; `--file
target/perf-log.jsonl` for the local log), marking a value over its
budget.

`clax-core`, `clax-rt`, `clax-mcp`, `clax-server` and `clax-content-git`
also build for `wasm32-unknown-unknown`, the target a Worker runs Rust as. `just
check-wasm` (`scripts/check-wasm.sh`) checks them there and fails on any
warning; arguments, when given, go to `cargo check` in place of that crate
list. The target alone decides what is built (`cfg(target_arch =
"wasm32")`, no cargo feature); `clax-server`'s crate documentation says
what the daemon alone runs and how the server reaches its runtime through
`clax-rt`. Bundled SQLite
for that target is C built by a clang with a wasm32 backend, which Apple's
clang lacks: the script takes `$LLVM_BIN`, else Homebrew's LLVM (`brew
install llvm`), else the `clang` on `PATH`, else the newest Debian or
Ubuntu `/usr/lib/llvm-<N>`, and fails saying what is missing (that LLVM, or
the target: `rustup target add wasm32-unknown-unknown`). `.cargo/config.toml` sets the
`getrandom_backend="wasm_js"` cfg for the target. On it the store runs in
single-connection mode (`Store::open_on`), and what needs a local machine is
built on unix only: the tool-call journal's real file system
(`toolpath::segment::StdFs`), git discovery (`gitctx::find_git` finds none),
`file://` links in exports, and file modes on the home's directories and
private files. The gates run this check in a lane of their own (`wasm32`),
so a machine running them needs that LLVM and the target.

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

### The Clax app

`apps/clax_app` is a Flutter app for Android and macOS. It runs `clax serve`
on the device, with a home of its own under the app's support directory
(`clax-home/`) on a free loopback port, shows each app full screen in a
WebView, and runs its own agent: the Claude API with your key, or, in a
desktop debug build, Claude Code. Its design is
`docs/superpowers/specs/2026-10-12-flutter-app-design.md`, with
`2026-10-12-flutter-app-round2-design.md`,
`2026-10-12-flutter-real-apps-design.md` and
`2026-10-13-flutter-app-bundles-design.md` beside it; what it relies on
from the daemon and the shell is in `docs/contract.md` ("The Clax app's
agent", "The Clax app", "Sharing apps", "Device capabilities in the Clax
app", "Reading another app's documents", "Fetching from the network",
"Security model").

**Toolchains.** Flutter (stable; 3.47 is what it is built with), Xcode for
macOS, and for Android a JDK 17, the Android SDK (platform 36, build tools,
platform tools, command-line tools) and NDK 28.2.13676358. Point Flutter at
them once, and accept the SDK licences:

```
flutter config --jdk-dir "$(ls -d ~/Library/Java/jdk-17*/Contents/Home | head -1)"
flutter config --android-sdk ~/Library/Android/sdk
flutter doctor --android-licenses
flutter doctor
```

`scripts/build-app-clax.sh` takes the NDK from `ANDROID_NDK_HOME`, else
`~/Library/Android/sdk/ndk/28.2.13676358`, and adds the Rust targets
`aarch64-linux-android` and `x86_64-linux-android` itself.

**Recipes.**

- `just app-clax [android|macos|all] [--debug]` builds `clax` for the app
  and places it where the app's build picks it up: the host binary at
  `apps/clax_app/build-native/macos/clax`, and stripped Android binaries at
  `apps/clax_app/android/app/src/main/jniLibs/{arm64-v8a,x86_64}/libclax.so`
  (all git-ignored). A release build (the default) embeds `web/dist`, so it
  builds the web UI first. Each binary is renamed into place, never copied
  over one that may be running.
- `just app-test` runs the app's gate, `apps/clax_app/tool/check.sh`:
  `flutter analyze`, the unit and widget tests (against a debug `clax` kept
  by content, for the tests that start the daemon), and
  `scripts/sync-app-prompt.sh --check`, which checks that the prompt assets
  in `apps/clax_app/assets/prompt` still copy `docs/contract.md`'s "Page
  contract" and "Runtime capabilities" sections and the served type
  definitions. After changing either section, run
  `scripts/sync-app-prompt.sh` and commit the assets with it.
- `just app-integration` builds the macOS `clax` and runs the integration
  tests on macOS, one `flutter test <file> -d macos` per file of
  `integration_test/`: the full loop (a scripted agent making and changing
  an app on the real daemon and inspector) and the credential-gated daemon.
  Run it in the background; it builds into `apps/clax_app/build/macos`.
- `just app-eval [--model ID] [--prompts a,b] [--max-spend USD] [--run
  NAME]` is the live build eval, run by hand and never by a gate: each
  prompt of `apps/clax_app/tool/eval/prompts.json` is built by the app's
  own agent on the Messages API, against a scratch daemon, to the end of
  its build. It needs `ANTHROPIC_API_KEY` and spends real money: up to $6 a
  build, refused before anything runs when that times the prompts is over
  `--max-spend` ($30 by default). Each run's files, screenshots, plan,
  per-run costs and a results table go to `target/flutter-app/eval/<run>/`.
  It builds the macOS debug app, so do not run it beside another macOS
  build or `just app-integration`.
- `just app-apk` builds the Android binaries and a release APK, at
  `apps/clax_app/build/app/outputs/flutter-apk/app-release.apk`. It is
  signed with the debug key, which is enough to install it on your own
  phone.
- `just app-macos` builds the macOS `clax` and a release (sandboxed) app,
  at `apps/clax_app/build/macos/Build/Products/Release/clax_app.app`.

No test and no gate calls a model: the agent is tested against a scripted
backend and a fake Messages API server. Runs against a live model are
manual. A debug build given `--dart-define=CLAX_SCRIPTED_AGENT=<file>`
answers with the scripted results in that file (a JSON array of edit
results, or `{"error": "<message>"}` items) instead of a model, which drives
the whole app without one.

**The kit.** The agent's `add_kit` copies `apps/clax_app/assets/kit/`
(see its `README.md`) into an app; `kitApi` in
`lib/agent/tools/kit_tool.dart` describes it to the model, in the tool's
result and in the system prompt. The map part (`map.js`) loads its tiles
through `net`; `web/e2e/map.spec.ts` drives it in the app's view against
tiles the test answers at the shell's `net:fetch` request, the consent
dialog included.

**The home app.** The home screen is an app of its own, built from
`apps/clax_app/home/` (`index.html`, `home.js`, `home.css`, its tests, and
`manifest.json` naming its title, capabilities, lineage, `version`,
`release` and the kit files it carries) into the bundle the app ships,
`apps/clax_app/assets/home/home.clax`, with its release record
`assets/home/release.json`. On the first run the app installs it and
records its ID in `home.json`; at each start it updates the installed home
app from a newer shipped release (silently, unless the person changed it:
then they choose Update or Keep mine, once per release). The `home`
capability it uses is "The home screen's capability" in
`docs/contract.md`; `assets/kit/fake-home.js` stands in for it in its
tests (`add_kit` part `home`).

The bundle is signed by the home app's own key, and every release must be
signed by the same one: an install treats a bundle under another key as a
different app. Its seed is kept outside the repository, owner-only, at
`~/.config/clax/home-bundle/key` on the owner's Mac (`CLAX_HOME_BUNDLE_KEY`
names another file), so **only the key holder can pack a release**. To
release a change to `home/` or to the kit files it carries, raise
`version` and `release` in `home/manifest.json`, then run:

```
./scripts/build-home-bundle.sh   # a dry run: says what it would write
just home-bundle                 # packs and writes home.clax and release.json
```

and commit both outputs with the sources. Packing unchanged sources writes
nothing; packing changed files under the shipped version, or without the
key, stops and says why. `test/home/home_bundle_test.dart` fails when
`home/` changed and was not packed. A kit edit does not fail it: the
bundle keeps the kit copies it was packed with, and the test reports the
drift as a skipped test naming the files, until the key holder packs the
next release.

**On an Android phone.** Turn on developer options and USB debugging, plug
the phone in, and from the checkout run:

```
export PATH="$HOME/Library/Android/sdk/platform-tools:$PATH"
adb devices    # lists the phone; accept the prompt on it if it says unauthorized
just app-apk
adb install -r apps/clax_app/build/app/outputs/flutter-apk/app-release.apk
```

`flutter run --release -d <device>` from `apps/clax_app` builds, installs
and starts it in one step, once `just app-clax android` has placed the
binaries. The app needs Android 10 (API 29) or later on an arm64 or x86_64
device. Open it, tap Settings on the launcher (a banner says so while no
key is set), and paste a Claude API key into "Claude API key"; it is kept in
the phone's Keystore-backed secure storage and sent only to the Claude API.
Pick a model there too (Sonnet 5.5 by default; Haiku 5.5 is faster,
Opus 5.5 stronger). Then New: say or type what
you want. The app's daemon, its documents and every version stay on the
phone; only the model call leaves it. Other apps on the phone can reach
its port, but the daemon answers only the app's own WebViews and agent,
which hold its token (see "Security model" in `docs/contract.md`).

**On a Mac, with Claude Code instead of a key.** A sandboxed app cannot
start `claude`, so the Claude Code backend is offered only in a debug
build, which runs without the App Sandbox:

```
just app-clax macos
cd apps/clax_app && flutter run -d macos   # or: flutter build macos --debug
```

In Settings choose "Claude Code on this Mac" (it uses your Claude Code
login; set "Claude Code path" when `claude` is not on `PATH` or in
`~/.local/bin`). Each change runs `claude -p` with tools off, at effort low,
with `--safe-mode`, so your CLAUDE.md, memory, skills, plugins, hooks and
MCP servers stay out of it. The release build (`just app-macos`) is
sandboxed and offers only the Claude API with a key, kept in the login
keychain.

**Signing the Mac app, so the keychain stops asking.** By default the
macOS app and its embedded `clax` are signed ad hoc: each build is a new
signature, the keychain item trusts only the build that saved it, and
macOS asks "clax_app wants to use your confidential information stored in
'flutter_secure_storage_service'" after every rebuild (once a launch: the
app reads the key once a process). With a free Apple Development
certificate (Xcode → Settings → Accounts → add your Apple Account →
Manage Certificates → + Apple Development), sign with it instead:

```
security find-certificate -c "Apple Development" -p | openssl x509 -noout -subject   # OU= is your team ID
sed 's/XXXXXXXXXX/<team ID>/' apps/clax_app/macos/Signing.xcconfig.example > apps/clax_app/macos/Signing.xcconfig
just app-macos
```

`apps/clax_app/macos/Signing.xcconfig` is gitignored; without it builds
stay ad hoc. On the first launch of a signed build the keychain asks once
more, since the saved item still lists only the ad-hoc build: click Always
Allow (or save the key again in Settings). After that, rebuilds keep the
same signature and do not ask.

**Using the app.**

- **Home.** Your apps in a grid, each with the icon its agent designed;
  the inbox button with its unread count; Settings; and New. A long press
  on a tile offers Open, Inspect and, while its plan has features left,
  Resume build. The home screen is itself an app: comment on it (Settings
  › Home screen › Open) to change it, as any app, for example "sort my
  apps alphabetically"; it never lists itself. When it cannot show (it
  fails to load, or has not shown within 10 seconds of a load of its
  page: the first at a start, and the reload after each Undo or Reset),
  Clax's built-in home screen takes its place with "Your home screen
  couldn't load", **Undo last change** and **Reset** (Clax's own home
  screen again, as a new version, your data and threads kept). Undo
  makes the version before the latest current again, as a new version;
  each Undo in a row steps back one more (from versions 1, 2 and 3, Undo
  restores 2, and Undo again 1), even across a restart, until there is
  no earlier version. Any other change (a new version from a comment, a
  Reset or an update) starts it again from the latest. Undo restores the
  files only: the home app's capabilities stay as they are. Settings ›
  Home screen has Open, Undo last change, Reset to Clax's home screen,
  and Use the built-in home screen.
- **Making an app.** New, then say or type what you want. The build view
  shows the plan as a live checklist, what the agent is doing, and a live
  preview of the page as it is written and rendered. Add, edit, drop or
  reorder features while it builds. Open app is offered as soon as the
  first version lands; the agent goes on building the rest of the plan,
  a version at a time, until it is done, the build has spent $6, or you
  Stop it (Resume carries on). Back to apps (Android's back, or Esc and
  ⌘[ on macOS) leaves a make running: up to three run at once, each a
  tile with its progress until it lands, and a tap shows its build view
  again. While a make or a change runs with Clax in the background,
  Android keeps a progress notification up, then posts "<title> is ready"
  (or that it changed, or couldn't); its tap opens the app.
- **No connection.** Comments and new apps are kept on the device at once,
  whether or not the Claude API can be reached. A make, a change, Keep
  going or Retry that cannot reach it waits instead of failing: the make's
  tile reads "Offline" and its build view "Waiting for connection", and a
  change says "Waiting for connection" on its Working chip and its thread,
  which offers Try now and Cancel (Cancel stops that comment alone; the
  thread then reads Cancelled, with Send again). While something waits and
  Clax is in front, it probes the API, every few seconds at first and less
  often the longer it stays down, and at once when Clax comes back to the
  front or you tap Try now; the work goes on by itself when it answers,
  also after Clax restarts. A run that loses the connection part way
  pauses and resumes from the turn it was on, without repeating what it
  already did; waiting does not count against its time limit. A turn
  whose connection breaks more than five times waits for Try now or for
  Clax to come back to the front, and each broken attempt that reached the
  API counts toward the run's turn and cost limits.
- **Commenting.** Nothing of Clax's is drawn over an open app. On Android,
  a silent notification named after the app stays up while it is open,
  with **Comment** and **Threads**; on macOS, App › Comment (⇧⌘K) and App ›
  Threads (⌘K). A two-finger swipe left opens the threads too, and when
  notifications are off, or you swiped the notice away, a thin handle on
  the right edge does. A first-run hint says where these are. Comment
  mode frames the page and puts a bar under it: "Tap anything to comment
  on it", **Comment on the whole app** (a comment on no element), Threads
  (with the open count), Voice (whether a pick starts dictation) and ✕.
  Tap what you want changed to comment there; hold, then drag, to select
  text or draw an area. Clax's composer docks at the bottom (over the
  page, which keeps its size, and above the keyboard) and shows what the
  comment is on: the screenshot of what you picked beside its words, the
  selected text, or a "Whole app" chip. Dictate or type, then Send (or
  Cancel, or Add to plan while the app builds); the threads show each
  comment's screenshot too. The page stays
  usable while the change runs: the Working chip narrates it and folds
  into a small pill at the top right, just under the status bar and any
  camera cutout, and a tap opens the change in progress (its
  comments, steps and latest render, Stop, and Open thread).
- **New versions.** A new version of the open app (a change, a build's
  next feature, an update) never replaces it by itself: a toast "New
  version ready" offers **Reload**, which swaps it in place on the page
  and route you are on, with your data and scroll kept. While you are
  writing a comment the toast waits until you send or cancel it; if it
  fades, it comes back the next time you return to the app. A new app you
  just made opens on its first version directly. A toast says when Clax replied, and
  View opens the thread at its reply; a thread's page takes replies, shows
  Retry on a failed change and opens the version that addressed it.
- **Big changes.** A change has one run of 10 steps, 3 checks and 2
  minutes. A request too big for that (a new screen, several features)
  becomes a short build plan instead: the reply says so and the Working
  chip shows the build, as for a new app. A change that still runs out
  says which limit it hit ("too big for one go — it needed more than 10
  steps"), with the agent's note of what is left when it gave up for
  lack of room, and offers **Keep going**:
  it goes on from the stopped run's unfinished draft and notes, or, after
  an hour, starts over with the notes. A change and its Keep goings may
  cost $6 together; the reply says what is left when under half. A build
  that spent its $6 waits; Resume on it starts a new $6.
- **Questions.** When the agent asks you something while its app is on
  screen, the question docks in a sheet at the bottom, with a light tap
  (back or Esc closes it before anything else). Over another app a toast
  says so, and View shows it; on the home screen, the build view or the
  inbox it opens in a sheet; with Clax in the background a notification
  "<title> has a question" shows it when tapped. A second question waits
  for the first to close.
- **Inbox.** Replies, new versions, new apps, questions and finished work
  across your apps, newest first and live: Unread or All, filters by kind,
  search, Mark all read; a tap opens the thread, version, app or
  question.
- **Versions.** The threads panel's Versions tab lists every version:
  tap one to preview it, Restore (or Undo) to make an earlier one's files
  current again; the app's capabilities stay as they are. A version's
  page shows the threads it addressed and the files it changed, with a
  line diff against the version before, or against the current one with
  Compare with current.
- **Inspector.** Inspect on a tile's long press shows what an app is made
  of: Overview (title, ID, versions, size, capabilities, history), Files
  (read-only), Data (collections and documents as JSON), Permissions
  (reads of other apps, the agent's remembered read consents, device
  capabilities, network hosts and AI use, each revocable) and Plan.
- **Location.** The first time an app asks where you are, Clax offers
  Precise, Approximate (only your area, about 2 km across) or Don't allow,
  and keeps the answer until you revoke it in the inspector's Permissions.
  An app may follow your location only while it is on screen: a small
  location pill shows at the top left meanwhile, a tap on it stops it, and
  leaving the app stops it for good until the app asks again from a tap.
  When Android will not ask again, or the phone's location is off, a toast
  offers Open settings. Location is Android-only for now; on the Mac apps
  are told it is not available.
- **Sharing an app.** Share app (Export… on the Mac) on a tile's long
  press or on the inspector's Overview makes the app a `.clax` file and
  hands it to the share sheet (on the Mac, a save panel). The sheet says
  which version goes and how many files; **Include my data** (off by
  default, and unavailable past 2,000 documents or 8 MiB) adds the app's
  documents, so the other person starts with them. Never in the file: your
  API key, your other apps or their data, comment threads, earlier
  versions, or the permissions you granted. The first share of an app
  makes your sharing key and calls it v1; sharing again after a change
  makes the next version, and sharing an unchanged app again sends the
  same version. Settings › Sharing holds **Your name**, shown to the
  people you share with, and, once you have shared, your sharing key's
  fingerprint (`7KQ2-M9XD-4FTA-QW3B`).
- **Installing one.** Open a `.clax` file in Clax: from Files or a
  message on Android (Open with, or Share to Clax), by double-clicking it
  in Finder on the Mac, or with Import app… in the launcher's overflow
  menu on either. The install sheet shows the app, who it is from (their
  name as they gave it, which anyone can claim, beside their key's
  fingerprint: check the fingerprint with them another way when it
  matters), what it can ask for (each capability in words, the internet
  hosts by name) and whether it comes with documents. Install, and the
  app opens. It needs no API key to install or use; changing it, or an
  app that uses Claude, does. A file that was damaged or changed after it
  was signed, or that is not a Clax app, is refused with one sentence.
- **Updates.** An update is a later file of the same app from the same
  sender; there are no automatic checks. Opening it offers Update (with
  what the new version also asks for), and the update is checked before it
  lands: Clax runs it against a copy of your data (its data upgrade, its
  page and its tests), and only when all pass does it become the app's
  next version, with your documents kept and the earlier versions still
  in Versions. When the check fails, "Your app is unchanged", and Details
  says why. While an update runs, the app shows "Updating…"; if its data
  upgrade then fails on your documents, the app goes back to the version
  before (files, title, icon and permissions) with your documents as they
  were, your own private ones included. An update cut short by Clax quitting is put back at the next
  start, or, when the app changed since, Clax asks whether to Restore or
  Discard. If you changed the app yourself, choose **Update (replaces
  your changes)** or **Keep mine (stop updates for this app)** (for the
  home app, **Keep mine (skip this update)**: the next release asks
  again). A file of
  an older or the same version is not installed; the same app signed by
  someone else's key, or the original of an app you made your own,
  installs as a separate app beside yours. Sharing an app you installed
  sends the sender's file as you received it; if you changed it, you may
  instead share **your version as a new app**, which you then update
  yourself.
- **Back.** In an app, back (Android's back gesture or button, Esc or ⌘[
  on macOS) first closes what is over the page (the questions sheet, the
  composer, the threads panel, the comment bar), then goes back through
  the app's own pages and routes, and only from where the app started
  returns to your apps. Elsewhere it goes back a screen. On macOS Esc in a
  text field only leaves the field, and while an app's page has the
  keyboard only ⌘[ goes back.

The app keeps its own records in its application support directory
(inside `~/Library/Containers/dev.empathic.clax` for the sandboxed macOS
build): `settings.json`, `timings.jsonl` (say → usable, long-press →
landed, home-usable and home-cover times, also listed in Settings),
`agent-timings.jsonl`,
`lineages.json` (which apps were shared or installed as files, and under
which key), `home.json` (the home app's ID, whether the built-in home
screen is chosen, the shipped release the person kept their own home
app over, and where Undo last change has stepped back to),
`bundles/<ID>.clax` (each installed app's file as received, for sharing
it on), `snapshots/` (an app's documents while an update
upgrades them, put back if it fails or Clax ends part way), and
`clax-home/`, the daemon's home. The sharing key is in the platform's
secure storage beside the API key; uninstalling Clax loses it, and your
later files then install on others' devices as separate apps. It never touches `~/.clax` or port 7480.

Its cache directory keeps `home/home-snapshot.bin`: the last image of the
home app's page, with where its app tiles were. A cold start shows it at
once, under the home app's page as it loads, until the page says it has
drawn the home screen; a tap on it then opens the app whose tile it fell
on, if that app is still there.

**Measuring the home screen's start.** Two timings run from the app's
launch (its `main`): `home-usable` ends at the home app's first `shown()`
and `home-cover` at the first frame of the kept snapshot (none on a first
run, or after the cache was cleared). Each launch adds one line of each
to `timings.jsonl`. On the phone:

```sh
just app-phone        # build and install the release APK
adb shell am force-stop dev.empathic.clax
adb shell am start -W -n dev.empathic.clax/dev.empathic.clax_app.MainActivity
```

Each `am start` after a `force-stop` is a cold start; Settings lists the
last ten timings ("Launch → home usable", "Launch → home snapshot").

A first launch after an install has no snapshot yet; leave the home
screen up for a second (the snapshot is taken shortly after it is
drawn, and whenever it is covered or put away) before the next launch.
On macOS, `CLAX_APP_DIR=<dir inside ~/Library/Containers/dev.empathic.clax/Data>`
in the environment of the app's executable
(`clax_app.app/Contents/MacOS/clax_app`) gives it a scratch support and
cache directory, so a measured build leaves your own apps alone.

## Releasing

A release is what the plugins download, and what `install.sh` installs.
Only a person cuts one. Its downloads work only while the repository is
public.

```
scripts/bump-version.sh 0.4.0     # every version, both Cargo.lock files and the skills' tool blocks
just bazel-sync                   # bazel/workspace.bzl and MODULE.bazel.lock follow the new version
just ci
git commit -am "Release 0.4.0"
git tag -s v0.4.0 -m "Clax 0.4.0"
git push origin v0.4.0            # the tag alone: main moves once the release is pinned
# wait for the Release workflow to publish v0.4.0, then:
scripts/pin-release.sh v0.4.0     # PINNED_VERSION and the four SHA256 values, in every wrapper copy
just ci
git commit -am "Pin the plugins to Clax 0.4.0"
git push origin main
# once CI passes on that commit: Actions, Publish Pi package, Run workflow (dry_run off)
```

The Pi package, `@empathic/clax-pi` on npm, is published from the pin
commit, not the tag: its wrapper must pin the release that shares its
version, which `.github/workflows/publish-pi.yml` checks before it
typechecks, packs and publishes with provenance through npm trusted
publishing. npm lets a package name a trusted publisher only once it
exists, so the first version is published by hand from `plugins/pi`
(`npm login`, then `npm publish`; `publishConfig` makes it public), and
the package's settings on npmjs.com then name this repository and
`publish-pi.yml`. After that first publish, `README.md` and
`plugins/pi/README.md` lead their Pi install with `pi install
npm:@empathic/clax-pi`. `scripts/test-plugins.sh` keeps the package
publishable: public, tagged `pi-package` for Pi's gallery, Pi's own
modules as optional `"*"` peers (Pi provides them; installed, they would
add a second Pi of about 200 MB), no `.qual` files, and a `LICENSE`.

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

### The crates.io package

`cargo install clax` is not published yet; everything up to the upload is
in place. crates.io takes a package holding only its own files, and the
binary needs eleven workspace crates and the built web UI, extension and
plugins, so `scripts/crate-package.py` assembles one package, `clax`, in
`target/crate-package/clax`. The workspace crates the binary depends on
become modules of it under `crates/<name>/src/`, their `crate::` and
`clax_<name>::` paths rewritten to match; the files the sources and
`clax-embedded` include are copied at their repository paths; the
dependencies are the union of the crates' own, with the workspace's
`Cargo.lock`; and `scripts/crate-package/build.rs` stands in for the
crates' build scripts. Its license is `GPL-2.0-only AND AGPL-3.0-only`:
GPL-2.0 Rust code with the AGPL-3.0 web UI embedded.

```
just crate-package                            # build the web UI, assemble, pack, and check the packed crate (uploads nothing)
cd target/crate-package/clax && cargo publish --allow-dirty   # the upload, by a person, from the release tag
```

`just crate-package` checks the packed crate as `cargo install` would
unpack it, with `cargo check --locked` into `target/` (no binary, so
`target/debug/clax` is untouched), and is not part of `just ci`; run it
before a release. The package records the commit it was
assembled from in `BUILD_COMMIT`, so `clax --version` names it.

## Security model

Clax traffic on the LAN is not encrypted: serve on the LAN (`clax serve --bind 0.0.0.0`) only on a network you trust.

Writes, and reads of the session list (working directories, process IDs, harness session IDs), need the token in `~/.clax/daemon.json` (mode 0600, served only to localhost browsers), or a connected person's machine key. A LAN viewer who is not connected can read artifacts and comment on them, but not publish or change them. A connected person's agents publish, reply, resolve and write page data with that person's rights, as the owner set them. They sign every request with their machine's key, which never leaves that machine, so a request seen on the network cannot be replayed. Connecting machines, approvals, rights and raises change only in the owner's or the person's own browser: no agent, tool or CLI command can change them. What another person wrote reaches an agent only once that agent's person approves its author. An agent of the owner's that has a shell runs as the owner's OS user, and is trusted as the owner. Every `/api` route answers only to a `Host` of `localhost`, `127.0.0.1`, `[::1]`, or the exact IP and port the connection arrived on (403 `forbidden_host` otherwise), which defeats DNS rebinding. The comment routes need no token but refuse requests from another origin, including published pages. The viewer cookie never leaves the daemon; viewers are named by a public ID, comment threads carry no session IDs, and the daemon's `codex` path is served only with the token. Published content is isolated on `<id>.localhost` origins or sandboxed. Content and asset URLs are fetchable by anyone who knows the unguessable ID.

Runtime capabilities arrive in a later phase.
