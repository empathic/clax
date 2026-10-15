# Clax on Cloudflare: a git platform for artifacts people and their agents work on together

**Source:** this repository, Apache-2.0 (`LICENSE`, `NOTICE`). The Worker is
`crates/clax-worker`; its [README](../../crates/clax-worker/README.md) covers
developing, the bindings and deploying. Run the whole demo locally, without a
Cloudflare account, with `just hosted-demo`. Video script:
[`video-script.md`](video-script.md). Designs:
[`2026-10-11-clax-dev-hosted-design.md`](../superpowers/specs/2026-10-11-clax-dev-hosted-design.md)
(hosted Clax, live collaboration) and
[`2026-10-08-multi-agent-multi-person-design.md`](../superpowers/specs/2026-10-08-multi-agent-multi-person-design.md)
(people, rights, the visibility filter).

## The problem

Agents produce more and more of what a team reviews: briefs, plans,
prototypes and reports, written as HTML pages. Several people, each with
their own agents, work on the same page, and three things break:

- Concurrent agents overwrite one another, because a write does not say
  which version it was based on.
- An agent that reads a teammate's comment takes the teammate's words as
  instructions: prompt injection between people's agents.
- Nobody sees who is working on what until it lands.

## What Clax is

Clax started as a local daemon: an agent publishes an HTML artifact over
MCP and gets a link, people comment on the page in the browser and send
threads to an agent, and the agent answers with a new version. Every
version is kept. This entry runs the same Rust server as a hosted instance
on Cloudflare, where several people sign in and each brings their own
agents:

- **Versions in git.** Each artifact is a git repository, and each version
  one commit on `main`, tagged `v<n>`, with `Clax-Version`,
  `Clax-Publisher` and `Clax-Agent` trailers naming who published it and
  through which agent. People clone it with git or jj, and a push to
  `main` becomes the next version.
- **Concurrent agents.** A publish names the version it was based on
  (`if_version`). A stale one is refused with `version_conflict`, carrying
  the current version; the agent merges its change into that and publishes
  again. A pushed commit takes part in the same rule: an agent working from
  the version before the push conflicts and merges onto it.
- **Each agent hears only its own person.** Every field an agent reads
  passes one visibility filter (`crates/clax-core/src/visibility.rs`):
  words written by anyone other than the agent's own person are withheld
  until that person approves the author, for one thread, one artifact or
  everywhere. The agent is told that a comment is held and by whom, never
  what it says, and can ask its person. Approving is the person's alone,
  from their browser; an agent's request to approve is refused.
- **People's rights.** Each person holds rights (read, publish, comment,
  send to agents, create) on artifacts named by ID or on their own
  (`crates/clax-core/src/rights.rs`); an agent acts with its person's
  rights through an agent token. Pages can be shared by link for viewing
  or commenting, with guests who give a name.
- **Live.** People on an artifact see each other's pointers, selections
  and comments as they are typed, anchored to the page's text. Agents say
  what they are doing (the `working` tool), shown as chips and a working
  line, and show their work in progress with the `draft` tool: Show draft
  renders it in the page with the changed elements highlighted.
- **Live seats.** Each artifact has live seats: by default 100 for people
  (guests hold at most 20 of them) and 20 for agents. The owner and the
  people an artifact names always get one. A person beyond the seats is a
  read-only spectator, polling a snapshot shared between spectators for 5
  seconds, and is offered Join live when a seat frees.
- **Forks.** A person who can read an artifact and create their own can
  fork it from any version; the fork names its source and version.

## How it uses Cloudflare

`crates/clax-worker/wrangler.jsonc` declares every binding.

- **Workers.** The Worker's fetch handler (`src/lib.rs`) serves the web
  UI's entry pages and `/_clax/` files from Static Assets with the daemon's
  headers, hands `/git/…` to the repository code, and sends everything else
  (the JSON API, `/mcp`, `/a/…` with its first-load data, artifact content,
  the WebSocket upgrades) to the instance's Durable Object. Artifact
  content is served on a host of its own (`CONTENT_ORIGIN`), sandboxed,
  never on the app's origin.
- **Durable Objects.** One `InstanceDo` (`src/instance.rs`) is the whole
  Clax server for an instance:
  - **SQLite through a VFS.** The daemon's database, schema and migrations
    unchanged, opened through `clax-do-vfs` (`crates/clax-worker/vfs`),
    which keeps the database file as fixed-size chunks in the object's
    `ctx.storage.sql` and hands each committed transaction's chunks to
    storage in one `transactionSync`. Journals live in memory; the store
    only ever holds the database as of a committed transaction.
  - **Hibernating WebSockets.** The event stream, the live layers and the
    page's `room` capability are WebSockets accepted with the hibernation
    API. Each socket's attachment carries the credentials it was opened
    with (never a token or a cookie), so an object woken from hibernation
    reopens each as the same caller, checked against the store again. Live
    seats and presence are kept in storage across a wake.
  - **Alarms.** The daemon's background loops (presence and working
    records lapsing, drafts going idle, idle live seats released, grant
    expiry, a question's grace) run from the object's alarm, set for the
    earliest one due; with nothing due there is no alarm, and an idle
    object sleeps, open sockets or not.
  - A second class, `RepoDO`, keeps the local git stand-in's repositories
    and the repository token ledger.
- **Artifacts** (`src/repos/artifacts.rs`, with `REPO_STORE=artifacts`):
  - one repository per artifact, `a-<artifact ID>`;
  - one commit per version, written by the Worker over git's receive-pack
    protocol with a write token it mints for itself and caches (the
    binding has no write call), sending only new objects: unchanged files
    and directories keep their blobs and trees;
  - reads through `readCommit`, `readTree` and `readBlob`, refs from the
    remote's advertisement;
  - forks as new repositories holding the history up to the forked
    version (an Artifacts fork would copy every object of the source,
    which Clax cannot prune afterwards), with readiness waits for a
    repository Artifacts is still making;
  - repository tokens for people and agents through `createToken` and
    `revokeToken`, recorded in `RepoDO`'s ledger;
  - pushes: the Artifacts event subscription delivers
    `cf.artifacts.repo.pushed` events to the `clax-artifacts-events`
    **Queue**, whose consumer asks the instance to record the pushed
    commits as versions, acknowledging on success or refusal and retrying
    failures. Before every publish the instance also records any commits
    still unrecorded, so a lost event never blocks one.
- **R2.** Artifact assets (`src/repos/assets.rs`), in both store modes.
  They are kept out of git on purpose: they are unversioned and deletable,
  and a deleted asset must be gone, which a git history cannot promise.
- **Static Assets.** The web UI (`web/dist`, the Svelte shell and the page
  bridge), with `run_worker_first`.

Locally, `REPO_STORE=memory` keeps real git objects (byte-identical
hashes) in `RepoDO`, and the Worker serves them over git smart HTTP at
`/git/` for clone, fetch and push, with the same version layout,
conflict rules and push recording as against Artifacts.

## One codebase, two hosts

The local daemon and the Worker build the same crates; the target decides
what is compiled (`cfg(target_arch = "wasm32")`), and `just check-wasm`
checks the shared crates for `wasm32-unknown-unknown` with warnings denied.
The design note at the top of `crates/clax-server/src/lib.rs` lists the
split in full.

Shared, compiled for both:

- `clax-core`: the store (SQLite schema, migrations, every query), people,
  rights and grants, the visibility filter and approvals, versions and
  forks, threads and feedback, drafts and their diff, working records,
  presence and live seats, behind a `ContentStore` interface for version
  files and assets.
- `clax-server`: the router, every request handler and the middleware, the
  stream hub, the live layers, the room hub, seats, hosted sign-in
  (setup, invites, sign-in links, guests, agent tokens) and push
  recording. On the Worker the MCP endpoint is a JSON-RPC adapter over the
  same tool set (`clax_mcp::http`); the daemon serves it at `/mcp/json`
  beside rmcp's streamable HTTP service, and the MCP tests run against
  both.
- `clax-mcp`: the MCP tools agents call (`publish`, `read`,
  `wait_for_feedback`, `comments_reply`, `draft`, `working`, `fork` and the
  rest).
- `clax-git` and `clax-content-git`: git objects, packs, a receive-pack
  client and an upload-pack/receive-pack server in pure Rust with no I/O of
  their own, and the version layout over any repository host.
- `clax-rt`: the runtime seam (tasks, timers, clocks): tokio natively, the
  Worker's event loop on wasm32.
- `web/shell` and `web/bridge`: the UI, including the comment picker
  (elements, text ranges and areas).

Worker only: `crates/clax-worker` (request routing, `InstanceDo` and its
sockets and alarm, `RepoDO`, the VFS, the Artifacts and R2 hosts, the queue
consumer). Daemon only: its lifecycle, SSE, rmcp's transports, process
spawning, and the files-on-disk content store.

## Several agents at once

The demo (`demo/agents.ts`, over `/mcp` with one person's agent token) runs
three scripted agents that write one product brief, one section each. Each
round they all start from the same version; each says what it is doing
(`working`), sends drafts (`draft`), and publishes with `if_version`. The
first lands and the others get `version_conflict` with the version that
won, merge their own section into it and publish again; each version's note
says when it took a merge. They then wait for comments
(`wait_for_feedback`): a comment their person sends is answered with a
revision, a reply and a resolve, and a comment another person sends is
withheld until their person approves that author. The agents call no
model; their text is fixed.

## Status

Runs under `wrangler dev --local`, in hosted mode, on the Rust stack:

- `just hosted-demo`: two people live, three agents drafting, conflicting
  and merging, a comment answered, another person's comment held and then
  approved, the spectator path, forks, a git clone, and a push becoming a
  version. `web/e2e/hosted-demo-worker.spec.ts` (`CLAX_E2E_WORKER=1`)
  checks most of it end to end: the two people's pointers, the agents'
  drafts, conflicts and versions, a comment answered and one held, the
  spectator joining live, the seeded fork, and the clone's commits and
  trailers. The rest (Show draft, approving in the browser, the Fork
  button, a push) has been driven in a browser against the demo, as the
  video script's recording notes list.
- `crates/clax-worker/e2e-hosted.mjs`: setup, an invite, a sign-in link,
  an agent token on `/mcp` held to its person's rights, a guest, content on
  its own origin, live pointers, a spectator, and seats across a simulated
  wake.
- `crates/clax-worker/test-repos.mjs`: clone with git and jj, tokens
  revoked and refused, pushes with git and jj becoming versions, version
  tags protected, assets in R2, across a restart.
- `crates/clax-worker/test-wasm.mjs`: the SQLite VFS across a restart.

Has never run against the real platform (the README's "What has not run
against the real platform"):

- Cloudflare Artifacts (`REPO_STORE=artifacts`) has no local simulator, so
  `src/repos/artifacts.rs` has run only against its tests' doubles. The
  error codes it retries on, the tokens of a repository deleted and made
  again under one name, and the cost of each version read are unverified.
- The pushed-event Queue consumer has never received a real Artifacts
  event; the event's shape is as the code reads it.
- Hibernation itself: the tests simulate a wake by rebuilding the server on
  the same storage; a real eviction also drops in-flight requests.
- Custom domains, certificates and `CF-Connecting-IP` from Cloudflare's
  edge.
- The Worker build on Linux: `build.sh` has run on macOS only.

Git tokens are minted from the browser: the artifact's Clone dialog asks
`POST /api/artifacts/<id>/git-token` for a token of the person's rights
(read for readers, write for those who may publish; read-only for anyone
reading a page through its link), and the demo's clone and push tokens
come from it too.

A TypeScript prototype came first and has been retired; this entry is
the Rust stack.

## Running it

Prerequisites: the Rust toolchain (`rust-toolchain.toml` adds the
`wasm32-unknown-unknown` target), Node.js, `npm ci` in `web/` and in
`crates/clax-worker/`, and on macOS Homebrew LLVM (`brew install llvm`),
since SQLite's C sources need a clang with a wasm32 backend. Then, from the
repository root:

```sh
just hosted-demo
```

It builds the web UI and the Worker, starts a fresh `wrangler dev --local`
in hosted mode on a free port, seeds it, and prints sign-in links for two
people, an invite for a third, an agent token with its `claude mcp add`
command, the git and jj clone commands, and the steps to try; Enter starts
the demo agents. `just hosted-demo --native` runs the native daemon in
hosted mode instead, and `--port`, `--seats`, `--agents`, `--pace` and
`--no-build` adjust the run.

To connect a real agent, use the printed `claude mcp add --transport http
clax http://localhost:<port>/mcp --header "Authorization: Bearer cxa_…"`
command; it acts with Rae's rights.

Deploying (the bindings, vars, secrets, the two origins' DNS and the steps
after the deploy) is in `crates/clax-worker/README.md`, Deploy to
Cloudflare.

## Next

Run against Cloudflare Artifacts and Queues on a deployment; mint git
tokens from the browser; sync a laptop's Clax daemon with a hosted one, so
an agent on a laptop and a teammate on the web work on the same artifact;
filtered clones per reader instead of all-or-nothing tokens; merging a fork
back.
