# Remote instances: read, watch and reply on any Clax's artifacts

Date: 2026-10-29. Status: approved by the owner.

## Intent

An agent handed a Clax artifact URL from any instance should work with it
as fluidly as with one on the person's local daemon (`localhost:7480`).
Instances include the hosted Worker on a laptop (`just clax-dev-local`,
`http://localhost:8788`), `https://clax.dev`, and an on-prem hosted
daemon. Working with it means reading the page, watching it, and reading,
replying to and resolving its comments, with new comments arriving in
`clax feedback follow` like local ones. The person's only step is approving
the connection, once per instance, on that instance's own page.

Every instance takes the same path, identified by its origin. Nothing
names clax.dev.

## How it works today (verified in the code)

- **Discovery exists.** `GET /.well-known/clax`
  (`crates/clax-server/src/routes/discovery.rs`) is open on every instance,
  the Worker included. It returns
  `{kind, contract_version, sign_in:[…], capabilities}`.
  - It does not list device sign-in (`/api/hosted/device/*`).
  - Its doc comment promises a `worker` kind that it never returns.
- **A signed-out `/a/<id>`** is a bare JSON 404 from
  `crates/clax-http/src/hosted.rs` (`guest_may`, :1459), whatever the
  `Accept`. Only `/a/<id>/get` has an HTML refusal with Sign in
  (`crates/clax-server/src/shell/get_page.rs`).
- **Connecting a remote** (`clax remote add`, the daemon's
  `routes/remotes.rs`) is a device flow:
  - The daemon gets a code from the remote's `POST /api/hosted/device/code`.
  - The person approves at the remote's `/device?code=`.
  - The daemon polls `/api/hosted/device/token` and stores an agent token
    in `remotes`, one row per origin (`clax-store/src/remotes.rs`).
  - `add`, `poll` and `remove` call `refuse_agents()` and
    `require_owner`. Approving mints a credential that the home holds.
- **A connected remote's token** is used for pushing (`public.rs`) and for
  linked-threads sync (`crates/clax-server/src/linked.rs`).
  - Linked-threads sync runs one task per remote over the remote's public
    REST API: it creates a session, watches, long-polls `feedback`, and
    reads, posts, resolves and reopens threads. It keeps a local twin's
    threads in step with the remote's.
  - It covers only artifacts pushed from this home (`remote_links`).
- **The MCP tools and the CLI** resolve every `url_or_id` through one
  function, `clax_mcp::target::target` (`crates/clax-mcp/src/target.rs:41`).
  - A URL on the local daemon's host and port is an artifact.
  - Anything else is a plain web page (`Target::Page`), so
    `read` answers "no live page … yet".
  - `watch` creates a local live-page placeholder ("No snapshot yet") for
    it (`routes/watches.rs` `live_put`).
- **Layering** (`scripts/check-crate-layers.py`):
  - `clax-mcp` depends only on core and cannot reach the store or tokens.
  - `clax-server` holds the only remote REST client
    (`public::Client`).
  - So remote calls go through the local daemon.

## Decisions the owner made

1. **Controlled initiation.** An agent may ask to connect an instance, but
   it cannot put a sign-in request in front of the person by itself, so
   there is no spam from any agent that runs. The person also must not
   have to start every connection from the web.
2. **One hint on every 404.** Every `/a/<id>` 404 says the same thing,
   whether the artifact is missing or private: it may be private, here is
   how to sign in, and agents should connect this instance. Because the
   text never varies, it reveals nothing about whether a page exists.

3. **No trust for reaching the port.** No local process or OS user is
   trusted at the owner's level because it can reach the daemon over
   loopback or the LAN. Every such hole is closed first, without hurting
   ergonomics (qualifier `c3cd21b0`).
   - Today `GET /api/token` hands the owner token to any loopback caller,
     and the owner's browser cookies are derived from the token and minted
     against headers a non-browser can forge.
   - Connected remotes would carry that power off the machine, so this
     comes before everything else.
   - The fix and its one residue: the plan's Task 0
     (`docs/superpowers/plans/2026-10-29-remote-instances.md`). The residue
     is that an agent running as the owner's OS user can read
     `daemon.json`; only OS sandboxing closes that.

## Design

### 1. Discovery

`/.well-known/clax` gains:

- `name`: the instance's display name, from hosted config, or its host.
- `origin`: its public origin.
- `api`: the REST base, `"/api"`.
- A `device` entry in `sign_in`, with its endpoints
  (`/api/hosted/device/code`, `/device`, `/api/hosted/device/token`).

The Worker and hosted daemons already share `routes/discovery.rs`. The
fix to the `worker` kind's doc comment goes with it.

Discovery also gains `site_domain` when the instance has one. A
remote's artifact URLs are then:

- `<origin>/a/<id>`, with `/get`, `/v/<n>` and other subpaths
- `<origin>/c/<id>/v/<n>/…`
- `<id>.<site_domain>`

All of these resolve to the remote. *My call.*

The local daemon caches a discovery answer per origin: negative answers
for 10 minutes, positive ones until a request fails. That way a plain web
URL is fetched once, not on every tool call.

### 2. One resolver, in the daemon

New daemon route: `GET /api/resolve?url=<url>`. It may be called by an
agent; it reads and changes nothing. The answer is one of:

- `{"kind":"local","artifact_id":…}`: the daemon's own artifact.
- `{"kind":"remote","remote":<id>,"origin":…,"artifact_id":<remote id>}`:
  an artifact on a connected remote.
- `{"kind":"sign_in_required","origin":…,"name":…,"request":{…}}`: a Clax
  instance this home is not connected to. `request` says how to ask the
  person (section 3).
- `{"kind":"page","url":…}`: not a Clax instance. It is handled as a live
  page, exactly as today.

The order is local, then connected remote (by origin), then discovery
(cached), then page. `clax_mcp::target` keeps its local fast path, the
URL on the daemon's own base. For any other URL it asks the daemon,
instead of assuming a page. Every tool already goes through it
(`resolve_ref`/`resolve_id`, `do_watch`), so `read`, `watch`,
`comments_*` and the CLI's `clax read`/`comments` all get the resolver.

### 3. Controlled initiation: connection requests

Initiation is split from approval. `add`, `poll` and `remove` stay as
they are, the owner's alone, `refuse_agents` unchanged. A new, narrower
thing is open to agents: a **connection request**.

- **Asking.** `POST /api/remotes/requests {origin, reason?, url?}` may be
  called by an agent, but only with a harness session registered on this
  daemon (an `X-Clax-Session` or a path session). A bare daemon token with
  no session is refused, so a process that is not a connected agent cannot
  ask.
- **Gate checks** before the person hears anything:
  - The origin passes discovery: it is a Clax instance, HTTPS, or loopback
    over HTTP.
  - It is not already connected. If it is, the answer is
    `{"status":"connected"}`.
  - It is not muted.
- **Folding.** There is at most one open request per origin. A second ask
  from any agent joins it: the request records each asking session and
  the URL each wanted, and answers with the existing request.
- **Rate limit.** A new request for an origin the person dismissed with
  "Not now" is refused for 24 hours. Each session may open at most 3 new
  requests a day. Times come from the store's injected clock.
- **Muting.** "Never for this instance" mutes the origin until the owner
  unmutes it in Settings, Remotes. A muted origin's request answers
  `{"status":"muted"}` and nobody is asked.
- **Telling the person.** Once, when the request opens, the daemon puts
  it in front of the person the way `where_to_ask` and `ask` already
  place a question: the gallery's inbox, and the asking agent's thread
  when it was working in one. The card reads "<agent> wants to connect
  to <name> (<origin>) to read <url>", with three buttons: **Connect**,
  **Not now**, and **Never for this instance**.
- **Connect** runs the owner's existing `add` from the person's browser
  session: the device flow, then the remote's approval page in a new tab.
  The person approves there, as today. The final yes is always the
  person's, on the remote.
- **Waiting for the answer.** The agent waits with
  `GET /api/remotes/requests/<id>?wait=` (a long-poll like
  `wait_for_feedback`). Possible answers:
  - `connected`, with the remote's ID
  - `dismissed`
  - `muted`
  - `expired`: open requests expire after 24 hours

An MCP tool `connect_instance {url_or_origin, reason?}` wraps the request
and the wait. The `sign_in_required` resolve answer, and every hint the
tools give, name that tool. The CLI gets `clax remote request <url>`. The
agent check in `clax remote add` stays; its message gains "agents: use
`clax remote request <url>`".

The agent never sees the device code or the token.

### 4. Remote artifacts: read, watch, comment

All through the remote's public REST API with the stored agent token, in
`clax-server`, built on `public::Client` and `linked.rs`'s calls:

- **read**:
  - `GET /api/artifacts/{aid}` and
    `/versions/{n}/files/{path}`, proxied by a new daemon route
    `GET /api/remotes/{remote}/artifacts/{aid}[/…]`.
  - The MCP `read` tool returns the same shape as for a local artifact,
    with `url` the remote's URL and `remote: {origin, name}` added.
- **comments_read / reply / resolve**: proxied the same way, to the
  remote's `/threads`, `/comments`, `/resolve` and `/reopen`. The tool
  sends an `Idempotency-Key`.
- **watch** adds the remote artifact to that remote's linked-threads task
  as a *followed* link, not a pushed one.
  - **A thread mirror.** A followed remote artifact gets a local
    `artifacts` row of a new kind, `remote_mirror`, holding the remote
    artifact's threads and comments and nothing else: no versions, no
    page, no files.
    - The task keeps its threads in step with the remote, as it does
      for a pushed twin.
    - A comment arriving there therefore takes every delivery route a
      local comment takes: notices in `clax feedback follow`,
      `wait_for_feedback`, tool results, hooks, push, with
      acknowledgement and resend.
    - The page itself is always read from the remote. A mirror is
      hidden from the gallery, lists and search.
    - Unwatching by every session drops the followed link, and the
      mirror after 7 days.
    - *My call.* A mirror is the only way to get every route without a
      second delivery pipeline. It copies threads, not pages, so it is
      not the twin rejected below.
  - **One remote session per local session.** Each local session
    watching a remote artifact gets its own session on the remote, named
    after the local agent. A person on the remote can then send a
    comment to that agent, as they can locally.
    - The task routes what arrives for each remote session to the
      matching local session.
    - A local session's end ends its remote sessions.
    - This also settles concern `7c8c51dd` for followed links.
    - *My call, from the goal of the same fluidity as local.*
- **Which link wins.** A URL that is the public copy of an artifact this
  home pushed resolves to the local artifact. Its twin already carries
  the threads, so it is never followed as well. *My call.*
- **Which credential.** Connect stores the agent token that `add` makes
  today, so a remote's comments reach an agent under the remote's own
  approval rules. Comments the remote withholds are counted in `read`
  and `comments_read` ("2 comments from people you have not approved
  for agents on <name>"). It does not sign in as `clax-app` to get past
  them. *My call: the remote's privacy rules stand.*
- **Who may use remotes.** Only the owner and the owner's agents may use
  the resolver's remote answers, the remote routes and connection
  requests, matching pushes' `asker` (`routes/remotes.rs:64-82`). A
  connected person's agent gets `page` or a local answer only, because
  the token is the owner's. *My call; it is a security boundary.*
- **Not offered on remote artifacts in this design:** `publish`/update,
  `db_*`, `delete`, `tag`, `pin`, assets, `fork`, `git_remote`. Each
  answers a clear "not on a remote artifact yet" error naming the
  instance.

### 5. The 404 hint

Every 404 on `/a/<id>` and its subpaths, signed out or signed in, gets
the same hint. It covers missing, private and deleted artifacts alike:

- **JSON** (`ApiError` not_found for those paths):
  `{"error":{"code":"not_found","message":"not found",
  "hint":"This may be a private page. If it is yours, sign in at
  <origin>/signin. Agents: connect this Clax instance with the
  connect_instance tool or `clax remote request <url>`.",
  "sign_in":"<origin>/signin","instance":"<origin>/.well-known/clax"}}`
- **HTML** for `Accept: text/html`: a small page in the sign-in pages'
  chrome with the same words and a Sign in button.

The hint does not depend on whether the artifact exists or on the
caller. A test checks that a missing ID and a private ID produce
byte-identical bodies.

### 6. On-prem is first class

- A remote is its origin. HTTPS is required except on loopback
  (`clax_core::config::base_url_ok`, which already enforces this).
- Tokens stay per origin in `remotes`.
- New tables:
  - `remote_requests` (ID, origin, status, asking sessions, URLs,
    created/answered times)
  - `remote_mutes` (origin, until)
  - a `followed` kind on remote links

  They take one migration, the next free number on main (60 today; it
  renumbers if main moves).
- Discovery's cache is in memory.

## Goals and non-goals

- **Goal:** read, watch, comments_read, reply and resolve on a remote
  artifact, with notices in `clax feedback follow`, on the dev Worker,
  clax.dev and on-prem alike.
- **Goal:** an agent can get the connection started without spamming
  the person.
- **Non-goal:** publishing to, editing or deleting a remote artifact
  other than through `clax push`.
- **Non-goal:** offline copies of remote pages.
- **Anti-goal:** any path where an agent's call alone, without the
  person's click, connects an instance or reaches the person more than
  once per origin per day.
- **Anti-goal:** a 404 whose text depends on whether the page exists.

## Tests (fast, no sleeps, injected clocks, helpers from clax-testkit)

- **Resolver:** one case each for local, connected remote, unconnected
  Clax origin, and plain page; plus the negative-cache expiry on the
  injected clock.
- **Connection requests:**
  - asking without a session is refused
  - folding of a second ask
  - mute
  - the "Not now" back-off
  - the per-session cap
  - expiry
  - Connect leading to approved, denied and expired device flows
- **Remote read, watch, reply and resolve:** against a second in-process
  daemon in hosted mode acting as the remote. A comment there reaches the
  local session's notices.
- **404:** byte-identical JSON and HTML bodies for missing and private
  IDs, signed out and signed in.
- **End to end:** against `just clax-dev-local` (`web/e2e` or
  `demo/`-style):
  - the agent requests
  - the test approves as the person
  - `read` of a private page
  - a comment posted by the person reaches `clax feedback follow`

## Settled by the designer (calls to check)

- Thread mirrors for followed remote artifacts (section 4).
- One remote session per watching local session.
- A pushed artifact's public URL resolves to the local artifact.
- The agent token, under the remote's approval rules, with withheld
  comments counted.
- Remote use is limited to the owner and the owner's agents.
- Site-domain URLs resolve to the remote, and discovery publishes
  `site_domain`.
- `comments_handoff` works on remote artifacts like reply.
- A denied device flow answers the waiting agent `dismissed`.
- The 404 hint applies to hosted instances only. A local daemon's
  artifacts are the owner's own.
- An instance's display name comes from a new optional hosted `name`
  setting, falling back to the host.

## Left to the builder

- Route and table names.
- The card's exact copy.
- How the gallery shows open requests.
- Whether the remote proxy routes stream bodies or buffer them.
- How the followed link shares `linked.rs`'s task.

## Alternatives not taken (to record as qualifier `alternative`s)

- **Letting agents run `add` directly (lifting `refuse_agents`).**
  Rejected: any agent could then put an approval page in front of the
  person. `revisit:` if requests prove too slow in practice.
- **A full local twin per watched remote page**, as pushes have.
  Rejected: it copies pages the person does not own. The mirror copies
  threads only. `revisit:` if offline reading is wanted.
- **A second delivery pipeline for remote comments**, with no local row.
  Rejected: it would duplicate notices, hooks, push and resend.
- **One shared remote session for all local watchers.** Rejected: a
  person on the remote could not send a comment to one agent.
- **A different 404 for private and missing pages.** Rejected: it would
  leak whether a page exists.
- **Moving `public::Client` to `clax-core`, so that `clax-mcp` calls
  remotes itself.** Rejected: tokens live in the store, which `clax-mcp`
  cannot reach.
