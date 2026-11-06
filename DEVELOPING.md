# Developing Clax

How to run what you are changing: the dev daemon, an agent on your working
tree, clax.dev on your machine, and the two together. This page is the
how-to; [Developing Clax](docs/usage.md#developing-clax) in
`docs/usage.md` is the reference (the crate layering, every gate, the perf
benchmarks), [docs/bazel.md](docs/bazel.md) says which recipe runs what,
and [AGENTS.md](AGENTS.md) holds the rules coding agents follow here.

## Setup

Install [Bazelisk](https://github.com/bazelbuild/bazelisk) and `ibazel`
(`brew install bazelisk ibazel`) and [just](https://github.com/casey/just).
Every recipe then runs with Bazel, which fetches the rest; `just` alone
lists the recipes.

## Homes and ports

Each way of running Clax keeps its own data and port, so nothing you run
while developing touches the Clax your everyday agents use.

| What | Data | Port | Started by |
|---|---|---|---|
| Your everyday Clax | `~/.clax` | 7480 | your installed `clax`; never touched by the recipes below |
| The dev daemon, main checkout | `~/.clax-dev` | 7481 | `just watch`, `just dev <harness>`, `just serve` |
| The dev daemon, a linked worktree | `~/.clax-dev-<worktree directory name>` | its own, from 7482-7499 | the same recipes, run in the worktree |
| clax.dev on this machine | `target/clax-dev-local/` | the first free of 8788-8797 | `just clax-dev-local` |
| The remote end-to-end check | `target/remote-e2e/` | free ports | `just remote-e2e` |

Each worktree's dev daemon has its own home, so one branch's migrations
never upgrade the database another branch's build opens. A worktree's home
takes the first port no other dev home records and keeps it; the line
`clax dev: … CLAX_HOME=… (port …)` names both. `CLAX_HOME` and
`CLAX_DEV_PORT` still name another home or port when you want one.

## The dev daemon and web UI

```
just watch
```

Serves the checkout's dev home (http://localhost:7481 in the main checkout)
and rebuilds as you edit: a Rust change restarts the daemon, a web change
rebuilds the web UI (reload the page). Ctrl-C stops it. `just serve`,
`just stop` and `just doctor` run the checkout's build on the same home,
and `just clax` runs any other `clax` command there:

```
just clax remote list
```

## An agent on your working tree

```
just dev claude     # or codex, grok, pi; extra arguments go to the harness
```

Builds `clax`, puts it first on `PATH` for the session, starts the dev
home's daemon, and starts the harness on that home. When the build cannot
open the home's database, it stops before the harness and says why and what
to do. Claude Code and Pi load this checkout's plugin
(Claude Code with `--plugin-dir plugins/claude-code`, and your installed
`clax@clax` is off for that session); Codex and Grok cannot load a plugin
from a directory, so they run your installed plugin on the fresh build, and
`just install` is how to try plugin changes there. Start a new `just dev`
session to pick up plugin, skill or hook edits.

`just install` puts the working tree in front of your everyday agents, and
`just uninstall` takes it away.

## clax.dev on this machine

```
just clax-dev-local
```

Runs the hosted Worker (`crates/clax-worker`) under `wrangler dev --local`
with production's settings: GitHub sign-in against real GitHub, the Terms,
no test routes. It prints the address (http://localhost:8788, or the next
free port when another worktree holds it) and, after the first run, an
owner sign-in link.

- **The first run** registers a GitHub App for local sign-in (one button,
  then GitHub asks you to confirm). Its settings are kept in
  `~/.config/clax/local/github.env` and shared by every worktree.
- **Who you are there.** The owner is made through setup under the name
  Owner. Each run prints a one-use owner sign-in link (ten minutes); open
  it to be the owner. Sign in with GitHub instead to sign up as a new
  person, as a visitor would. `--owner` makes your GitHub account the
  owner, and `--setup` leaves setup open to try `/setup` itself.
- **Browsers share localhost's cookies across ports.** Use a separate
  browser profile, or a private window, for each person you sign in as.
- **Edits rebuild it.** A change under `web/` or `crates/` to a file git
  does not ignore (so not a build's own output) rebuilds the web UI and the
  Worker and restarts it on the same data; the line `A source changed (<file>): rebuilding.` names the file.
  `--no-build` serves the last build and watches nothing.
- **The landing page's reel** (`/start/`) builds in the background after
  the Worker is up (`target/clax-dev-local/reel.log`). The first build
  records every vignette and takes several minutes; until then the page
  shows no video. A rebuild cancels it and starts it again afterwards.
- **Data is kept across runs** in `target/clax-dev-local/`. `--fresh`
  deletes that directory and starts over.

## This branch's plugin against clax.dev on this machine

Two terminals, both in this checkout:

```
just clax-dev-local     # terminal 1: note its address, e.g. http://localhost:8789
just dev claude         # terminal 2: this branch's plugin on this checkout's dev home
```

Terminal 2's first line names the dev home and its port (7481 in the main
checkout, its own in a worktree); the dev daemon's inbox is at that port.
A browser sees it once signed in to that home: the first `just dev` on a
new home opens its gallery signed in, and `just clax open` (or `just clax
login` for a link) signs in another browser. A plain `clax open` reaches
your everyday Clax, not the dev home.

In the browser, open the owner sign-in link terminal 1 printed, so you are
signed in to clax.dev's address as the person the dev daemon will act as.
Then connect the dev daemon to it, either way:

- **As an agent would.** Ask the agent in terminal 2 to read a page there
  by its URL, or to connect to `http://localhost:8789`. A read answers
  `remote_sign_in_required`, naming `connect_instance`; the agent calls
  that, and a card appears in the dev daemon's inbox: "… wants to connect
  to …". Choose **Connect**.
  A new tab opens clax.dev's approval page; approve there. The agent's
  call answers `connected` and the read goes on.
- **By hand.** In a third terminal (not inside the agent's session: an
  agent cannot add a remote, and a command run from Claude Code's prompt
  counts as one):

  ```
  just clax remote add http://localhost:8789
  ```

  It prints a code and opens clax.dev's approval page; check the code and
  approve.

Once connected, from the dev daemon:

- the Publish button on an artifact's page in the dev daemon, or `just clax
  push <ID>`, publishes it there, and an agent's `publish_public` asks you
  first;
- an agent can `read` and `watch` pages there, and a comment written there
  reaches the agent as a local one does;
- `just clax remote list` shows the connection, and `just clax remote
  remove http://localhost:8789` ends it, revoking the token there.

`http` is allowed for a Clax on this machine; any other remote is `https`.
`docs/usage.md` ("Publishing to a public Clax", "Reading another Clax from
an agent") has the rest.

**The scripted version:** `just remote-e2e` builds everything, starts the
Worker and a daemon on a scratch home on free ports, and walks the agent
path above (connect, read a private page, watch, a comment arriving),
printing a line per step. It stops what it started and exits 0 when every
step passes.

## Tests and checks

- `just test-affected` while working: the tests downstream of your edits,
  chosen by the build graph (`just test-affected main` for the whole
  branch, `--list` to see them).
- `just check` once before a pull request: formats the Rust code, then runs
  every gate CI runs (`just ci` runs them without formatting).
- `just web-test`, `just pi-test`, `just worker-test` run one area.
- `just perf` runs the perf benchmarks on demand; they gate nothing.

## When something goes wrong

- **`just clax-dev-local` says the instance could not start** (`no such
  column`, `no such table`): an earlier build of a branch migrated its data,
  and a branch edits its own unmerged migrations in place, so that data is
  not upgraded. Start over with `just clax-dev-local --fresh`.
- **`just dev` says this build cannot open the dev home** (`could not
  upgrade the database from schema version …`): a build with other
  migrations upgraded it, such as an earlier build of this branch whose
  unmerged migrations have changed since, or, in the main checkout, a
  branch you ran there before. It names the home and stops before the
  harness. Move the home aside as it says (nothing is deleted, and moving
  it back restores it) and run `just dev` again.
- **The Worker did not come up.** The message says what `/healthz` last
  answered and shows wrangler's output from that run; all of it is in
  `target/clax-dev-local/wrangler.log` (every run's, appended), and the
  builds' in `target/clax-dev-local/build.log`.
- **The port is taken.** Another worktree's `just clax-dev-local` holds it,
  and a run takes the next free one; `lsof -nP -iTCP:8788-8797
  -sTCP:LISTEN` shows which. Stop processes by the PID you started, never by
  a name pattern.
- **"Sign-in failed: GitHub could not confirm who you are"** on clax.dev:
  GitHub refused the sign-in code (a code works once, so an attempt cut off
  by a restart spends it). Choose Sign in again. The Worker does not yet
  write the reason to its log.
