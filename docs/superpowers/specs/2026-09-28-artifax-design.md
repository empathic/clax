# Artifax: local Artifacts with comment-driven development for coding agents

Date: 2026-09-28
Status: draft for review

## 1. Purpose

Artifax replicates the Claude Artifacts experience on a developer's own
machine, for any coding agent. An agent publishes an HTML web app; the
developer opens it in a browser, leaves anchored comments on it, and sends
those comments back to the agent that published it. The agent acts on them,
replies, and resolves the threads. Pages written for claude.ai's runtime
(`window.claude.use(...)`) run unchanged.

Harnesses in scope for v1: Claude Code, Codex CLI, Pi. Each gets a thin
adapter; the substance lives in one shared Rust binary, following the layout
of toolpath and clash.

### Success criteria

- An agent in any of the three harnesses can publish, read, list, and update
  artifacts, and gets a stable local URL that outlives the session.
- A person can comment on an element or a text selection in a published page,
  hit "Send to agent", and the publishing agent receives the selector, the
  quoted text, the comment, and a PNG clip of the region, then replies and
  resolves the thread from inside its session.
- Pages using `artifact` (self-publish), `db`, `downloads`, `permissions`,
  `user`, and `comments` capabilities behave as they do on claude.ai.
- Everything works with no network access and no account.

### Out of scope for v1

- The `files` and `mcp` capabilities (they need claude.ai connectors and
  project channels). `claude.use("files")` and `claude.use("mcp")` resolve
  `null`.
- Multi-user identity and auth. See §14 for the single-user model.
- Artifact types (Slides, Docs, Design). Plain HTML pages only.
- Harnesses beyond the three named. The daemon's HTTP MCP surface makes
  others cheap to add later.

## 2. Decisions

Each decision has a one-line rationale. Contested ones are also listed in
§18.

| # | Decision | Rationale |
|---|---|---|
| D1 | Rust workspace, one `artifax` binary, web UI embedded in the binary | Matches toolpath and clash; single install, no Node at runtime. |
| D2 | Shared per-user daemon plus a per-session stdio MCP shim (Approach A) | Stable URLs and one gallery outlive sessions; the shim gives each session an identity. |
| D3 | Daemon speaks HTTP MCP too | Harnesses that prefer HTTP MCP can skip the shim later at no cost. |
| D4 | Storage: SQLite for metadata, comments, db docs, sessions; page content and assets as files on disk under `~/.artifax/` | Content is large and versioned; metadata needs queries and transactions. |
| D5 | Per-artifact origin via `<id>.localhost:<port>` when the shell is opened on localhost; opaque-origin sandboxed iframe when opened over LAN | Matches claude.ai's per-artifact origin isolation where the browser supports it; the fallback still satisfies the page contract (storage may be unavailable). |
| D6 | Runtime bridge is a script the daemon prepends to the page at serve time; it talks to the shell over `postMessage`, the shell talks to the daemon | Origin-agnostic, so D5's two modes share one code path. |
| D7 | Screenshot clips are rendered inside the content frame with a DOM-to-canvas library, not headless Chrome | No browser binary dependency; the clip is what the commenter actually saw. |
| D8 | Feedback delivery is tiered: tool-result piggyback, Stop hook re-engage, prompt-submit context, blocking wait tool, native push where a harness has one | No single mechanism works everywhere; the tiers degrade gracefully and are stated honestly in §10. |
| D9 | `watch` is armed automatically on publish for the publishing session, and only sent-to-agent comments ever reach an agent | Mirrors claude.ai; plain comments are for humans. |
| D10 | `sample()` calls the Anthropic Messages API with a configured key; provider is a trait | Simplest path to parity; other providers plug in later. |
| D11 | Write endpoints require a bearer token stored in `~/.artifax/daemon.json` (mode 0600); read and comment endpoints do not | LAN viewers can view and comment; only local processes can publish, delete, or administer. |
| D12 | MCP tool names mirror claude.ai's tools (`publish`, `read`, `list`, `comments_read`, `db_get`, ...) | Agents that already know the Artifact tools transfer that knowledge; skill files carry the contract. |
| D13 | Five phases, each shippable; phase 1 has no comments, no capabilities | Per the standing instruction; comments and capabilities are the churn-prone parts. |
| D14 | Pi adapter is specified against the published extension API and clash-pi, marked unverified on this machine | Pi is not installed here. |

## 3. Architecture

```
                 ┌─────────────────────────────────────────────────────────┐
                 │  artifax serve  (one per user, auto-started, port 7480)  │
   browser ──────┤  HTTP: shell UI, gallery, content, assets, REST API      │
   (shell +      │  SSE/WS: live events (versions, comments, room)          │
    content      │  MCP over HTTP (D3)                                      │
    frame)       │  SQLite + files under ~/.artifax/                        │
                 └───────▲──────────────▲──────────────▲───────────────────┘
                         │ HTTP+token   │ HTTP+token   │ HTTP+token
        ┌────────────────┴───┐  ┌───────┴────────┐  ┌──┴──────────────────┐
        │ artifax mcp        │  │ artifax hook   │  │ artifax <cli cmd>   │
        │ (stdio MCP shim,   │  │ (SessionStart, │  │ (publish, open,     │
        │  one per session)  │  │  Stop, Prompt, │  │  comments, doctor)  │
        └────────▲───────────┘  │  SessionEnd)   │  └─────────────────────┘
                 │ stdio        └───────▲────────┘
        ┌────────┴────────────────────────┴───────┐
        │ harness: Claude Code | Codex | Pi        │
        └─────────────────────────────────────────┘
```

Components:

- **Daemon** (`artifax serve`). Owns storage, serves the browser, emits
  events, hosts MCP over HTTP, runs `sample()` calls. Started on demand by
  any CLI or shim invocation that finds no live daemon; stays up until
  `artifax stop` or reboot.
- **Shim** (`artifax mcp --agent <claude|codex|pi>`). A stdio MCP server the
  harness spawns per session. On start it ensures the daemon is up,
  registers a session record, and proxies every tool call to the daemon
  over HTTP with the token. It appends undelivered feedback to tool results
  (§10 tier 1). It exits with the harness.
- **Hooks** (`artifax hook --agent <x> <event>`). Short-lived processes the
  harness runs at lifecycle points. They read the harness's JSON on stdin,
  call the daemon, and print the harness's expected JSON.
- **Shell UI**. The page at `/a/<id>`: header, version picker, comment mode
  toggle, thread sidebar, and an iframe holding the content. Also the
  gallery at `/`.
- **Content + bridge**. The published HTML wrapped in the document skeleton
  with `/_artifax/bridge.js` prepended. The bridge implements
  `window.claude.use`, comment-mode hit testing and highlighting, anchor
  resolution, and screenshot clips.
- **Plugins**. One directory per harness that packages the shim command,
  hooks, skills, and slash commands, plus an installer script that finds or
  downloads the binary (toolpath's `ensure-path.sh` pattern).

## 4. Repository layout

```
Cargo.toml                         workspace, edition 2024
crates/
  artifax-core/                    types, IDs, storage (SQLite + files), anchors, events
  artifax-server/                  axum HTTP/SSE server, REST API, MCP-over-HTTP, sample provider
  artifax-mcp/                     stdio MCP shim (rmcp), session registration, feedback piggyback
  artifax-hooks/                   per-harness hook protocol adapters (Claude, Codex, Pi JSON shapes)
  artifax-cli/                     binary `artifax`: serve, mcp, hook, publish, list, open, ..., doctor
web/
  shell/                           Preact + TypeScript: gallery, artifact shell, comment sidebar
  bridge/                          vanilla TypeScript: window.claude.use, comment mode, clips
  contract/                        the .d.ts files pages are written against (copied from claude.ai 0.2.61)
  dist/                            built assets, embedded into artifax-server via rust-embed
plugins/
  claude-code/                     .claude-plugin/plugin.json, .mcp.json, hooks/, skills/, commands/, scripts/
  codex/                           .codex-plugin/plugin.json, skills/, config snippets, setup skill
  pi/                              npm package @empathic/artifax-pi, extensions/artifax.ts
.claude-plugin/marketplace.json    Claude Code marketplace pointing at plugins/claude-code
.codex-plugin/marketplace.json     Codex marketplace pointing at plugins/codex (format to confirm, §18)
docs/superpowers/specs/            this document
docs/superpowers/plans/            one plan per phase
docs/contract.md                   the page contract and capability behaviour, for agents and humans
justfile, scripts/quality_gates.sh same gate style as toolpath
```

Dependency graph:

```
artifax-cli ── artifax-server ── artifax-core
           ├── artifax-mcp    ── artifax-core
           └── artifax-hooks  ── artifax-core
```

Key crates: `axum`, `tokio`, `tower-http`, `rusqlite` (bundled),
`rust-embed`, `rmcp` (official Rust MCP SDK), `serde`, `ulid`, `reqwest`
(shim, hooks, and the sample provider), `clap`. Web: Preact, Vite,
TypeScript, `modern-screenshot` for clips. No CSS framework.

## 5. Storage and data model

Root: `~/.artifax/` (override with `ARTIFAX_HOME`).

```
~/.artifax/
  daemon.json            {port, pid, token, started_at, bind}  mode 0600
  artifax.db             SQLite
  artifacts/<aid>/
    versions/<n>/index.html          the page as published (before wrapping)
    versions/<n>/files/<path>        supporting files for that version
    assets/<asset_id>.<ext>          asset store, shared across versions
    clips/<thread_id>.png            comment screenshot clips
  config.toml            bind address, port, sample provider, key env var name
  logs/daemon.log
```

IDs: artifact IDs are 12-character lowercase Crockford base32 from 60
random bits, so URLs read `/a/7q3k9mzx2b4t`. Thread, comment, asset, and
session IDs are ULIDs. Versions are integers starting at 1.

SQLite tables (abridged; columns beyond keys are illustrative):

- `artifacts(id, title, description, icon, created_at, updated_at,
  current_version, owner_session_id, pinned, capabilities_json,
  contract_version, deleted_at)`
- `versions(artifact_id, n, label, created_at, session_id, files_json)`
  where `files_json` maps published path to content type and size.
- `assets(id, artifact_id, content_type, ext, size, created_at)`
- `sessions(id, harness, harness_session_id, cwd, pid, parent_pid,
  started_at, last_seen_at, ended_at)`
- `watches(session_id, artifact_id, replies_armed, created_at)`
- `threads(id, artifact_id, version_n, anchor_json, status, sent_to_agent,
  created_at, resolved_at, resolved_by)`
- `comments(id, thread_id, author_kind, author_name, via_session_id, body,
  created_at)` with `author_kind` in `viewer | agent`.
- `feedback(id, thread_id, comment_id, target_session_id, created_at,
  delivered_at, delivery_tier, acknowledged_at)`; one row per (comment,
  target session). `target_session_id` is null when no live session was
  found at send time. `delivery_tier` is one of `piggyback | stop_hook |
  prompt_hook | wait | queue | inject`. `acknowledged_at` is set when the
  target session reads, replies to, or resolves the thread; delivered but
  unacknowledged rows are resent (§10).
- `docs(artifact_id, path, json, version, updated_at)` for the `db`
  capability; `path` is the full document path such as `tasks/t1`.
- `viewers(id, display_name, created_at)` for the `user` capability,
  keyed by a cookie.

Content addressing: every version keeps its own files; files omitted from a
later publish are copied forward as on claude.ai, `null` removes one. A
version is never rewritten.

## 6. HTTP API

All routes are on the daemon. Write routes (marked W) require
`Authorization: Bearer <token>`.

Browser-facing:

- `GET /` gallery shell. `GET /a/<aid>` artifact shell. `GET /a/<aid>/v/<n>`
  shell pinned to a version.
- `GET /c/<aid>/v/<n>/` wrapped content document (bridge prepended).
  `GET /c/<aid>/v/<n>/<path>` supporting files. Also served at
  `http://<aid>.localhost:<port>/v/<n>/...` for D5. On `<aid>.localhost` the
  daemon serves `/v/...`, `/healthz`, `/_artifax/*`, and `/_blob/*` and
  404s everything else.
- `GET /_blob/<asset_id>` asset bytes.
- `GET /_artifax/bridge.js`, `/_artifax/shell/*` static.
- `GET /api/events?artifact=<aid>` SSE stream: `version`, `thread`,
  `comment`, `doc`, `room` events. `artifact_deleted` is sent when an
  artifact is deleted, with the artifact's ID in its data. `resync`, with
  `data: {"dropped": n}`, is sent when a subscriber fell behind and `n` events
  were dropped; the client should refetch the state it displays.

Agent- and shell-facing JSON API under `/api`:

- Artifacts: `GET /api/artifacts`, `POST /api/artifacts` (W, create +
  version 1), `GET /api/artifacts/<aid>`, `POST /api/artifacts/<aid>/versions`
  (W; publish body below; `if_version` for conflict detection),
  `PATCH /api/artifacts/<aid>` (W: title, pinned, capabilities),
  `DELETE /api/artifacts/<aid>` (W), `GET /api/artifacts/<aid>/files`,
  `GET /api/artifacts/<aid>/versions/<n>/files/<path>` (a file's stored bytes,
  unwrapped; `Content-Security-Policy: sandbox`, `nosniff`),
  `POST /api/artifacts/<aid>/assets` (W or viewer-with-write-grant; see §9),
  `DELETE /api/artifacts/<aid>/assets/<id>` (W).
- Sessions: `POST /api/sessions` (W, register), `PATCH /api/sessions/<id>`
  (W, heartbeat or end), `GET /api/sessions`.
- Watches: `PUT /api/sessions/<sid>/watches/<aid>` (W), `DELETE` same (W),
  `GET /api/sessions/<sid>/watches`.
- Comments: `GET /api/artifacts/<aid>/threads`, `POST .../threads` (create
  thread with first comment; no token), `POST .../threads/<tid>/comments`
  (viewer: no token; agent: W and `author_kind=agent`), `POST
  .../threads/<tid>/send` (no token; sets `sent_to_agent`, creates feedback
  rows), `POST .../threads/<tid>/resolve` (viewer or agent).
- Feedback: `GET /api/sessions/<sid>/feedback?wait=<secs>` (W; long-poll,
  returns undelivered feedback for that session, marks delivered on return).
- Docs (db capability): `GET/PUT/PATCH/DELETE /api/artifacts/<aid>/docs/<path>`,
  `GET /api/artifacts/<aid>/docs?collection=<c>&where=...&order_by=...&limit=&cursor=`,
  `POST /api/artifacts/<aid>/docs:batch`. Versions enforce `if_version`.
  Access rules from the artifact's declared `db.rules` are evaluated per
  caller level (§9).
- Sample: `POST /api/artifacts/<aid>/sample` streams text over SSE.
- Room (phase 5): `GET /api/artifacts/<aid>/room` WebSocket.

Health: `GET /healthz` returns `{version, pid, started_at}` on every Host,
including `<aid>.localhost`; CLI, shim, and the D5 probe use it.

### Publish body

`POST /api/artifacts` and `POST /api/artifacts/<aid>/versions` take JSON:

```json
{
  "title": "Quarterly Review",          // optional after version 1
  "description": "…", "icon": "chart",  // optional
  "label": "Draft to legal",            // optional, ≤ 60 chars
  "if_version": 3,                      // required on an existing artifact
  "capabilities": {"db": {}},           // optional; omitted keeps, {} clears
  "files": {
    "index.html": {"content": "<title>…", "encoding": "utf8"},
    "app.js":     {"content": "…", "encoding": "utf8", "content_type": "text/javascript"},
    "logo.png":   {"content": "iVBOR…", "encoding": "base64"},
    "old.css":    null                  // remove a file carried forward
  }
}
```

`index.html` is required on every publish and is never carried forward.
Other files carry forward from the previous version unless given or
`null`. `content_type` defaults from the extension. The 64 MB cap is on
decoded bytes (the HTTP body limit is 96 MiB to cover base64 inflation) and
a single file is capped at 16 MB, as on claude.ai. Assets use multipart at
`POST /api/artifacts/<aid>/assets`.

## 7. Daemon discovery and lifecycle

1. A client reads `~/.artifax/daemon.json`, calls `/healthz` on that port,
   and checks the PID is alive. Match: use it.
2. Otherwise it takes an exclusive `flock` on `~/.artifax/daemon.lock`,
   re-checks, then spawns `artifax serve --daemonize` detached (new session,
   stdio to `logs/daemon.log`), and polls `/healthz` for up to 5 seconds. The
   spawning client holds the lock until `/healthz` answers.
3. `artifax serve` binds `127.0.0.1:7480` by default, tries the next 20
   ports if busy, and writes `daemon.json` atomically. `artifax serve` itself
   does not take the lock.
4. `artifax stop` sends `POST /api/admin/shutdown` (W). The daemon also
   exits if `daemon.json` is replaced by a newer daemon (checked every 30 s)
   and exits when `daemon.json` is missing on two consecutive checks, so a
   stale process cannot shadow a new one.
5. Version skew: the shim compares `/healthz` version with its own; on
   mismatch it asks the daemon to shut down and restarts it. Storage
   migrations run on daemon start.

## 8. Shell UI and viewer

Gallery (`/`): cards with title, description, icon, updated time, pinned
first; search; open, pin, delete; shows which session published each and
whether that session is live.

Artifact shell (`/a/<aid>`):

- Header: title, version picker (`v3 of 3`, older versions read-only), copy
  link, open raw content in a new tab, comment mode toggle, thread sidebar
  toggle, viewer display name.
- Content frame: iframe whose `src` is the per-artifact origin when
  reachable (D5 probe: fetch `http://<aid>.localhost:<port>/healthz` with a
  1 s timeout, cached per browser). In that mode the iframe carries no
  `sandbox` attribute; the distinct origin is the isolation. Otherwise the
  `src` is `/c/<aid>/v/<n>/` with `sandbox="allow-scripts allow-forms
  allow-modals allow-popups allow-downloads"` and no `allow-same-origin`,
  so the content runs in an opaque origin. Content on the main origin
  (`/c/...`) carries `Content-Security-Policy: sandbox allow-scripts
  allow-forms allow-modals allow-popups allow-downloads` and `/_blob/...`
  responses carry `Content-Security-Policy: sandbox`, so a top-level
  navigation cannot reach the API same-origin. A bare sandbox stops Chrome
  rendering PDFs top-level; assets are meant for `<img>`, `<video>`,
  `<a download>`, and fonts.
- Live updates: the shell subscribes to `/api/events`; a new version shows a
  "v4 published, reload" banner unless the page published it itself via the
  `artifact` capability, in which case it reloads immediately as on
  claude.ai.
- Thread sidebar: open and resolved threads, each with anchor summary,
  clip thumbnail, comments, "Send to agent" button, resolve. Clicking a
  thread scrolls the frame to its anchor and flashes it. Detached threads
  (anchor not found in this version) are listed under "Detached".
- Comment mode: the bridge highlights the hovered element with an outline
  and shows a floating pin cursor. Click selects that element; drag-select
  text creates a range anchor. The composer opens in the shell with the
  quote and clip preview.

The shell is Preact + TypeScript with CSS tokens on `:root`, dark mode via
`prefers-color-scheme`, phone width supported, so it follows the same page
contract it asks of artifacts.

Phase 1 builds the gallery, the header without the comment mode toggle,
thread sidebar, and viewer name, the content frame in both origin modes,
and the version banner. The comment affordances are added in phase 3 and
must not be stubbed into phase 1.

## 9. Runtime bridge and capabilities

The daemon wraps every version's `index.html` at serve time into the
document skeleton claude.ai uses (doctype, charset, viewport, the small
reset), inserts `<script src="/_artifax/bridge.js" data-artifact="<aid>"
data-version="<n>" data-contract="0.2.61">` as the first element of
`<body>`, then the page content. Recognition rule: if the file, after
whitespace and an optional BOM, begins with a `<!doctype` declaration
(case-insensitive), it is a complete document and is served as-is with the bridge script inserted
immediately after the first `<body ...>` tag; otherwise it is a fragment
and is wrapped. This is what makes a self-republished page (which sends
the full skeleton) round-trip without nesting. Wrapping is pure and cached
per version.

Capability ownership by phase. Every name below is placed; nothing else
exists in the surface.

| Capability | Phase | Notes |
|---|---|---|
| `use()` itself, resolving `null` for every name | 1 | The bridge ships in phase 1 so pages written against the contract load and degrade correctly. |
| `permissions` (built in) | 4 | Until phase 4, `permissions.state()` reports every capability unavailable and `request()` resolves the same. |
| `artifact`, `self` alias | 4 | |
| `db` | 4 | Together with the `db_*` MCP tools. |
| `downloads` | 4 | |
| `user` | 4 | Viewer cookie and display names arrive in phase 3 for comments; `user` exposes them in phase 4. |
| `comments` | 4 | Depends on phase 3 threads. |
| `assets` | 4 | The asset store and `asset_upload` tool are phase 1 and 2; page-side `assets` is phase 4. |
| `room` | 5 | |
| `sample` | 5 | |
| `files`, `mcp` | never in v1 | Resolve `null`. |

`window.claude` exposes only `use(name)` returning a Promise of a frozen
namespace or `null`. The bridge posts `{type:"artifax:use", name, id}` to
the shell; the shell answers with grant state. Unknown or undeclared names
resolve `null`. `use()` never rejects. A capability the artifact did not
declare in `capabilities` resolves `null` except `permissions`.

Capability behaviour, in the same shapes as the claude.ai 0.2.61 `.d.ts`
files kept in `web/contract/`:

- **permissions** (built in): `state()` and `request()`. Grants are
  per-viewer per-artifact, stored in the shell's `localStorage`; the first
  use of a consent-gated capability shows one shell dialog. Denial is final
  for the page load.
- **artifact**: `publish(html)` sends the complete document to
  `POST /api/artifacts/<aid>/versions` with `if_version` = the version the
  frame is showing; the shell attaches the token only if the viewer is the
  owner (the shell is on localhost). Conflict rejects `conflict` and the
  shell reloads to the winner. Read-only viewers (LAN) reject `not_writer`.
  `self` is an alias.
- **db**: `doc(path)`/`collection(path)` with get, set, update, delete,
  where, orderBy, limit, onSnapshot (over the SSE `doc` event). Rules from
  the declaration raise per-path minimums; caller level is `admin` for the
  owner shell on localhost, `interact` for a named LAN viewer, `view` for an
  unnamed one. Last-writer-wins with version pins; `acquire({holder})`
  single-writer lease with a 30 s TTL. `data/users/<id>/` is private per
  viewer ID.
- **downloads**: `save({filename, data})` triggers a browser download after
  a shell confirmation.
- **user**: `id()`, `me()`, `isOwner()`, `canEdit()`, `can(name)`,
  `profiles(ids)`, `search(q)`. Identity is the viewer cookie; names come
  from the `viewers` table; `guest` is `false` always.
- **comments**: `openComposer({element}|{range})` opens the shell composer
  anchored there; `customAnchors()` lets a page register named anchors for
  canvas content. Write verbs in the full form create threads and comments
  as the viewer. The shell renders all threads; the page never lists them.
- **assets**: `upload(blob)`, `list()`, `delete(id)`; owner shell only,
  `null` otherwise. Served at `/_blob/<id>`.
- **room** (phase 5): `emit`, `on`, `presence`, `onPeers`, `join(name)`
  over one WebSocket per frame; nothing persisted; topics gated by level.
- **sample** (phase 5): `sample(input, opts)` and `sample.json`, streaming
  `onText`, `tools` executed by round-tripping tool calls to the page,
  `modelTier` mapped to configured model IDs, `cache` as a 5-minute
  in-memory replay. First call asks consent in the shell. Provider trait
  with an Anthropic implementation; key from `config.toml` (`sample.api_key_env`,
  default `ANTHROPIC_API_KEY`). No key configured: `use("sample")` resolves
  `null`.
- **files**, **mcp**: resolve `null`.

The bridge also handles comment mode (hit testing, outline, text selection),
anchor creation and re-resolution, and clip rendering, on messages from the
shell. The bridge never trusts messages that do not come from its shell's
window and origin.

### Anchors

```json
{
  "kind": "element" | "range" | "custom",
  "selector": "main > section:nth-of-type(2) > h2",
  "quote": "Quarterly goals",
  "prefix": "...", "suffix": "...",
  "html_hash": "sha256:...",
  "rect": {"x":0,"y":0,"w":0,"h":0,"scrollX":0,"scrollY":0,"viewportW":0},
  "custom_name": null
}
```

Re-resolution order on a new version: exact `selector` with matching
`html_hash`; `selector` alone; text `quote` with `prefix`/`suffix` search;
`custom_name` for custom anchors. Nothing found: the thread is detached for
that version and stays attached to the version it was made on.

### Clips

On composer open the bridge renders the anchored element (for a range, its
nearest block ancestor) with `modern-screenshot` to a PNG at device pixel
ratio, capped at 1600 px on the long side, and posts the bytes to the shell,
which uploads them with the thread. Cross-origin images that taint the
canvas are dropped from the render; the thread still stores the anchor and
quote. The clip is saved at `~/.artifax/artifacts/<aid>/clips/<tid>.png` so
an agent can view it with its own file-reading tool.

## 10. Comments and the feedback loop

### Data flow

1. Viewer enters comment mode, picks an element or selection, writes a
   comment. Shell posts the thread with anchor and clip. Everyone with the
   shell open sees the pin via SSE.
2. Viewer presses **Send to agent** on the thread (or writes `@agent` in a
   comment). The daemon sets `sent_to_agent`, then creates one `feedback`
   row per target session: the artifact's owner session and every session
   with a watch on the artifact, if that session has not ended. If no live
   session exists, the feedback is stored with no target and is delivered to
   the next session that publishes a version of, or watches, that artifact.
3. Delivery happens by the tiers below. A feedback row is marked delivered
   once, by whichever tier delivers it first.
4. The agent calls `comments_reply` and `comments_resolve`. Replies appear
   as `Agent · via <harness>`. Only sent-to-agent threads accept agent
   replies and agent resolves; on a plain thread the tool returns guidance,
   mirroring claude.ai.

### Feedback payload

Rendered as text so it can be dropped into any harness:

```
[artifax] Comment sent to you on "Quarterly Review" (http://localhost:7480/a/7q3k9mzx2b4t), thread 01J9...
Anchored on: main > section:nth-of-type(2) > h2  «Quarterly goals»  (v3)
Clip: ~/.artifax/artifacts/7q3k9mzx2b4t/clips/01J9....png
Alex: "Make this a two-column layout and drop the third bullet."
Reply with comments_reply, then comments_resolve when done.
```

### Delivery tiers (D8)

| Tier | Mechanism | Harnesses | Latency | Failure modes |
|---|---|---|---|---|
| 1 | Shim appends undelivered feedback to every tool result it returns | all three | next tool call | Nothing arrives while the agent is idle or not using artifax tools. |
| 2 | Stop hook: if undelivered feedback exists for a watched artifact, output "block" with the payload as reason | Claude Code (confirmed shape), Codex (stop hook exists; block semantics unverified) | end of the current turn | Only fires when a turn ends; an idle session is not woken. Loop guard: a feedback row is delivered once, and the hook allows the stop when nothing new exists, honouring `stop_hook_active`. |
| 3 | Prompt-submit hook adds pending feedback as additional context | Claude Code (`UserPromptSubmit`) | the user's next message | Depends on the user typing something. |
| 4 | `wait_for_feedback` tool: long-polls the daemon for up to `timeout_s` | all three | immediate while waiting | Harness tool timeouts cap a single call (Codex defaults to 60 s), so the tool defaults to 50 s and returns "nothing yet, call again"; the skill tells the agent to loop while the user wants live feedback. |
| 5 | Native push | Codex: `codex queue --thread <id> --message` (below). Pi: extension message injection (API name unverified, §13). Claude Code: none available to third-party plugins. | seconds when the harness submits the message itself; otherwise the user's next input | See below, and the resend rule. |

### Delivery and acknowledgement

A feedback row is marked `delivered_at` by the first tier that hands it
to the harness. Delivery is not proof the agent saw it: a queued Codex
message or an injected Pi message can be dropped by the harness. So the
row also carries `acknowledged_at`, set when that session calls
`comments_read`, `comments_reply`, or `comments_resolve` on the thread, or
when tier 1 or tier 4 returns it (those paths are in-band and count as
seen). Rows delivered by tier 2, 3, or 5 and unacknowledged after 2 minutes
are included again by tier 1 on the next tool result and by tier 2 on the
next stop, marked `(resent)`. Resends stop after acknowledgement or after
three attempts; the thread then shows "delivered, not acknowledged" in the
shell.

### The Codex wake path, stated plainly

Mechanism: when a feedback row targets a Codex session whose
`harness_session_id` is known and whose watch has `replies_armed`, the
daemon runs

```
codex queue --thread <harness_session_id> --message <payload>
```

with the payload from above, a 10 s timeout, and the daemon's environment.
Exit 0 marks the row delivered with tier `queue`. The thread ID is the
`session_id` the Codex `session_start` hook received, joined to the shim's
session record by parent PID (§11).

What is known and not known as of this spec: the command exists in Codex
0.158 and is documented as "queue a message for an existing session". It
is not verified whether a queued message is submitted automatically when
the session is idle at its prompt, only submitted after the current turn
ends, or held until the user presses enter. Phase 3 measures this on the
installed Codex and records the result in `docs/contract.md`.

Latency by outcome:

- Submitted on idle: seconds. This is a true wake.
- Submitted at end of the current turn only: the end of the turn, the
  same as tier 2.
- Held until user input: the user's next message, the same as tier 3.

Failure modes and what happens in each:

- **Thread ID unknown.** The Codex hooks did not run (not installed, or
  Codex has not granted persisted hook trust), so no `session_start` hook
  registered the ID. The daemon skips tier 5 for that session, the `status`
  tool reports "Codex session ID unknown, native push disabled", and
  `artifax doctor --agent codex` explains how to install and trust the
  hooks. Tiers 1, 2, and 4 still apply.
- **Session has exited.** `codex queue` exits non-zero. The daemon marks the
  session ended, clears `target_session_id` on its undelivered rows, and
  delivers them to the next session that publishes a version of or watches
  the artifact. The shell shows "agent session ended; waiting for a new
  one".
- **`codex` not on the daemon's PATH.** The daemon was started by a process
  with a minimal environment. Tier 5 is disabled for all Codex sessions and
  `doctor` reports it. The shim passes its own `PATH` when it auto-starts
  the daemon, which covers the common case.
- **Queued but never surfaced.** Exit 0 but the agent never sees it. The
  acknowledgement and resend rule above catches it: the row is resent
  in-band on the next tool result or stop, up to three times.
- **Stop hook.** The Codex `stop` hook (`clash-codex/hooks.toml` shows the
  event exists) runs `artifax hook --agent codex stop`. Whether Codex
  honours a block decision from it is unverified; if it does not, the hook
  is a no-op and the tier is skipped on Codex.

Uniform fallback when nothing above fires: the feedback sits in the daemon
until the agent's next artifax tool call or the user's next message. The
shell shows "sent, waiting for the agent" with the elapsed time and the
tier it is waiting on, so the person knows the agent has not seen it.

The same honesty applies to Claude Code: without a first-party background
task, an idle Claude Code session is woken only by tiers 2 and 3. Live
feedback on Claude Code means the agent is in a `wait_for_feedback` loop.

### Watch semantics

`watch` rows are created on publish for the publishing session, by the
`watch` tool, or by the `/artifax:watch` command. `watch off` removes one.
A session's watches end when its `SessionEnd` hook fires or when the shim
exits and the daemon notices the closed connection. `replies_armed` mirrors
claude.ai's auto-reply arming and gates tier 2 and tier 5; tiers 1 and 4
work for any session with a watch.

## 11. Sessions and identity

The shim registers a session on start:

```json
{"harness":"claude","harness_session_id":null,"cwd":"/Users/alex/proj",
 "pid":48213,"parent_pid":48200}
```

Harness session IDs are often unknown to the shim (Claude Code does not
pass its session ID to MCP servers; Codex does not either). The hooks do
receive them. `artifax hook --agent <x> session-start` therefore registers
`{harness, harness_session_id, cwd, parent_pid}` and the daemon joins the
two records on `(harness, parent_pid)`, since both the shim and the hook
process are children of the same harness process. Where a harness exposes
the session ID in the shim's environment, the shim sends it and the join is
by ID instead. Claude Code additionally sets `CLAUDE_CODE_SESSION_ID` for
hook processes, as toolpath's plugin relies on.

Heartbeats: the shim `PATCH`es `last_seen_at` every 60 s; a session with no
heartbeat for 5 minutes and a dead PID is marked ended.

## 12. MCP tool surface

Exposed by the shim (stdio) and the daemon (HTTP). Names are shared across
harnesses; Claude Code shows them as `mcp__artifax__<name>`.

Artifacts: `publish` (file_path or html, files map, title, description,
icon, capabilities, url to update, if_version, label), `read` (url or id,
optional path), `list` (limit, scope mine|all, files scope), `delete`,
`open` (opens the browser on the daemon host), `pin`, `unpin`,
`asset_upload` (file_path or file_paths).

Comments: `comments_read` (url, thread_id, cursor), `comments_reply` (url,
thread_id, text), `comments_resolve` (url, thread_id), `watch` (url, on,
replies), `wait_for_feedback` (url optional, timeout_s default 50).

Data: `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`,
`db_str_replace`, `db_batch`, with `collection`, `doc_id`, `data`,
`file_path`, `if_version`, `as_level` as on claude.ai.

Server: `status` (daemon URL, version, this session's ID and watches).

Every tool result is JSON text plus, when present, a trailing
`---\n[artifax] N comments sent to you:\n...` block (tier 1). URLs in
results are always the daemon's browser URL so they can be pasted to a
person.

The daemon also exposes the same operations as `artifax <cmd> --json` for
harnesses without MCP and for scripts.

## 13. Plugins

### Claude Code (`plugins/claude-code`)

- `.claude-plugin/plugin.json`: name `artifax`, keywords, version.
- `.mcp.json`: `{"artifax": {"command": "${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh", "args": ["exec", "mcp", "--agent", "claude"]}}`.
- `hooks/hooks.json`: `SessionStart` → `hook --agent claude session-start`
  (registers, prints the daemon URL and any pending feedback as context);
  `UserPromptSubmit` → `hook --agent claude prompt` (tier 3); `Stop` →
  `hook --agent claude stop` (tier 2, timeout 10 s); `SessionEnd` →
  `hook --agent claude session-end`.
- `skills/artifax/SKILL.md`: when to publish, the page contract (title,
  tokens, dark mode, phone width, storage in try/catch, CDN guidance),
  capability usage with the `.d.ts` files as references, the
  comment-driven loop, and the `wait_for_feedback` loop convention.
- `commands/`: `/artifax:open [id]`, `/artifax:comments [id]`,
  `/artifax:watch [id] [off]`, `/artifax:wait [id]`, `/artifax:serve`
  (start, stop, status, bind for LAN), `/artifax:doctor`.
- `scripts/ensure-artifax.sh`: toolpath's `ensure-path.sh` adapted
  (`ARTIFAX_BIN`, `ARTIFAX_INSTALL_DIR`, GitHub release download with
  checksum, `exec` subcommand).
- Root `.claude-plugin/marketplace.json` lists it.

### Codex (`plugins/codex`)

- `.codex-plugin/plugin.json`: name `artifax`, `skills: "./skills/"`,
  `interface` block.
- `skills/artifax/SKILL.md`: same content as the Claude skill, with Codex
  tool naming.
- `skills/artifax-setup/SKILL.md` plus `scripts/setup.sh`: idempotently runs
  `codex mcp add artifax -- artifax mcp --agent codex` and appends the
  `[hooks.session_start]` and `[hooks.stop]` entries to `~/.codex/config.toml`
  (or a project `codex.toml`), following `clash-codex/hooks.toml`.
- `config.snippet.toml` and `hooks.snippet.toml` for manual install.
- If the Codex plugin manifest turns out to accept MCP server or hook
  declarations directly, phase 2 uses them instead of the setup skill (§18).

### Pi (`plugins/pi`), unverified on this machine

- npm package `@empathic/artifax-pi`, `pi.extensions: ["extensions/artifax.ts"]`,
  installed with `pi install npm:@empathic/artifax-pi`, following clash-pi.
- `extensions/artifax.ts`: on `session_start` runs `artifax hook --agent pi
  session-start` with the Pi session ID; registers tools through the
  extension API that call `artifax <cmd> --json`, or configures the stdio
  MCP shim if Pi's extension API exposes MCP registration; on `tool_result`
  appends pending feedback; for tier 5 uses the extension API's
  message-injection call if one exists.
- Pi is not installed on the machine this was designed on. Before the Pi
  tasks of the phase 2 plan run, install Pi and re-check each item below
  against the installed `@mariozechner/pi-coding-agent` types, recording
  the answer in the plan:
  1. Event names. `session_start`, `tool_call`, `tool_result` are confirmed
     by clash-pi; confirm they still exist and their payload fields
     (`event.sessionId`, `event.toolName`, `event.input`).
  2. Tool registration. Whether the extension API can register a tool with
     a JSON schema and an async handler, and what the return shape is.
  3. MCP registration. Whether an extension can register a stdio MCP server;
     if yes, the shim is used and item 2 is unnecessary.
  4. Message injection. Whether an extension can submit a user-role message
     into the running session, and whether that starts a turn when idle.
     This decides whether Pi has tier 5.
  5. Session ID propagation. Whether Pi exposes its session ID in the
     environment of spawned processes; if not, parent-PID join per §11.
  6. Install path. Whether `pi install npm:<pkg>` is still the install
     command and how `package.json`'s `pi.extensions` is read.
  7. UI. Whether `ctx.hasUI` and `ctx.ui.select` exist for a "watch this
     artifact?" prompt; optional.

## 14. Security model

- Default bind `127.0.0.1`. `artifax serve --bind 0.0.0.0` or
  `config.toml` opts into LAN. The gallery header shows the LAN URL when
  bound.
- Write endpoints need the bearer token from `daemon.json` (0600). Local
  shims, hooks, and the CLI read it; the shell on localhost fetches it from
  `/api/token`, which only answers to loopback connections and also requires
  a literal local `Host` header (localhost, 127.0.0.1, [::1]) to defeat DNS
  rebinding. LAN viewers can
  view, comment, send to agent, resolve, and write `db` docs at
  `interact` level; they cannot publish, delete, upload assets, or write
  `admin`-level docs.
- Content isolation per D5. In LAN mode content runs with an opaque origin.
  Content on the main origin (`/c/...`) carries `Content-Security-Policy:
  sandbox allow-scripts allow-forms allow-modals allow-popups
  allow-downloads` and `/_blob/...` responses carry
  `Content-Security-Policy: sandbox` (see section 8 for the PDF note), so a
  top-level navigation cannot reach the API same-origin.
- Comment bodies, doc contents, and room messages are untrusted data. Tool
  results wrap them in a clearly labelled block and the skills say so.
- `sample()` spends the configured key; consent is per viewer per artifact
  and the shell shows a running count of calls.
- No telemetry, no outbound calls except `sample()` and release downloads
  by the installer script.

## 15. Error handling

- Daemon unreachable after auto-start: shim tools return a structured
  error naming `~/.artifax/logs/daemon.log`; hooks exit 0 with no output so
  the harness is never blocked by artifax being down.
- Publish conflict (`if_version` stale): the tool returns the current
  version's content summary so the agent can merge, as claude.ai does.
- Anchor not found: thread marked detached, never dropped.
- Clip failure: thread saved without a clip; the payload says so.
- Hook timeouts: every hook finishes within 5 s or exits 0 silently, and
  the daemon call inside uses a 3 s timeout.
- `wait_for_feedback` past the harness's limit: returns early with a
  "call again" result rather than erroring.
- `codex queue` failure: handled per §10; never retried in a loop, never
  blocks the send request that triggered it (dispatch is asynchronous).
- Storage debris: `artifax doctor` reports it and `artifax doctor --fix`
  removes stray staging directories and temp files, version directories above
  the current version, zero-version artifact rows, and asset rows and corrupt
  rows belonging to soft-deleted artifacts. Rows of live artifacts are never
  deleted, and `--fix` refuses to run while a daemon is live.
- Request timeout (408 `timeout`): a 408 on a write route means the outcome
  is unknown; read the artifact before retrying.
- Storage corruption: `artifax doctor` runs `PRAGMA integrity_check`,
  verifies files against `versions.files_json`, and reports.

## 16. Testing

- **artifax-core**: unit tests for IDs, anchors (serialisation only; DOM
  resolution is tested in the browser), storage round-trips, rules
  evaluation for `db`, version copy-forward semantics.
- **artifax-server**: integration tests with a temp `ARTIFAX_HOME`, real
  SQLite, `axum` test client: publish, versions, files, assets, threads,
  send-to-agent → feedback rows, long-poll delivery, `db` rules by level,
  token enforcement per route.
- **artifax-mcp**: spawn the shim against a test daemon, drive it with an
  MCP client over stdio, assert tool schemas and tier 1 piggyback.
- **artifax-hooks**: fixture-driven tests with captured stdin JSON for each
  harness event and golden stdout, including `stop_hook_active` and the
  Codex `stop` shape.
- **Browser**: Playwright against a real daemon: gallery, shell, version
  banner, comment mode on element and range, clip produced, send to agent,
  agent reply visible via SSE, `window.claude.use` for each capability in
  both origin modes (`*.localhost` and opaque sandbox), db `onSnapshot`,
  self-publish reload.
- **Plugins**: shell tests for `ensure-artifax.sh`; a Claude Code smoke test
  that loads the plugin from the repo path and runs a scripted session;
  Codex setup script idempotency; Pi extension against a mocked
  `ExtensionAPI` until Pi is installed.
- `scripts/quality_gates.sh` runs fmt, clippy `-D warnings`, cargo test,
  web lint (oxlint) and typecheck, Playwright, and the plugin tests; CI runs
  the same script.

## 17. Phases

Each phase is shippable on its own and gets its own implementation plan.

**Phase 1: daemon, publish, versions, gallery, viewer.** Stands alone:
no comments, no sessions, no capabilities, no room, no sample. In scope:
`artifax serve` with discovery, auto-start, `daemon.json`, and `stop`;
storage with the `artifacts`, `versions`, and `assets` tables only (later
tables arrive with their phases via migrations); the REST routes for
artifacts, versions, files, and assets, `/api/token`, `/healthz`, and SSE
with the `version` event only; content serving and wrapping with the
recognition rule; the bridge with `use()` resolving `null` for every
name; the D5 origin probe and both frame modes; the gallery and the shell
per §8's phase 1 paragraph; the CLI `serve`, `stop`, `status`, `publish`,
`list`, `open`, `delete`, `pin`, `unpin`, `doctor`. `owner_session_id` is
null and the gallery shows "published from the command line". Ship when:
an HTML file with a supporting file can be published from the CLI,
opened at its URL, republished with `if_version`, its versions browsed
in the shell, and the version banner appears in an already-open tab.

**Phase 2: MCP and the three plugins.** Shim, session registration, hooks
for session-start/end, MCP tools for artifacts and `status`, HTTP MCP on
the daemon, `ensure-artifax.sh`, Claude Code plugin with skill and
commands, Codex plugin with setup skill, Pi extension (unverified), release
workflow producing binaries with checksums. Ship when: an agent in Claude
Code and Codex can publish and update an artifact through MCP, and the Pi
extension passes its mocked tests.

**Phase 3: comments and the feedback loop.** Threads, anchors, clips,
comment mode, sidebar, viewer display names, send to agent, feedback rows
with acknowledgement and resend, tiers 1–4, Stop and prompt hooks,
`comments_*` and `watch` and `wait_for_feedback` tools, tier 5 on Codex
via `codex queue` with its behaviour measured and written into
`docs/contract.md`, tier 5 on Pi if item 4 of the Pi checklist is
confirmed, the "waiting for the agent" indicator with tier and elapsed
time. Ship when: the loop in §1 works end to end in Claude Code, and the
Codex behaviour is measured and documented.

**Phase 4: runtime capabilities.** `permissions`, `artifact` and its
`self` alias, `db` with rules and snapshots and the `db_*` tools,
`downloads`, `user`, `comments` capability (composer and custom anchors),
`assets` from the page, the `.d.ts` contract files in `web/contract/`,
contract docs in `docs/contract.md`. Ship when: a claude.ai page using
these capabilities runs unchanged in both frame modes.

**Phase 5: room and sample.** WebSocket room with presence and topics,
`sample()` with the Anthropic provider, streaming, tools round-trip,
consent and spend counter, provider trait. Ship when: a two-tab room demo
and a `sample()` demo work with a configured key, and `sample` resolves
`null` cleanly without one.

## 18. Open questions and decisions to revisit

- **D5 `*.localhost` support.** Chrome and Firefox resolve `*.localhost` to
  loopback; Safari's behaviour needs checking in phase 1. The probe and
  fallback make this safe either way, but if Safari fails the fallback
  becomes the common case on macOS and per-artifact `localStorage`
  isolation is lost there.
- **Phase 3 cookie scoping.** The viewer cookie must be host-only and
  validated by the shell, since `<aid>.localhost` shares a site with
  `localhost`.
- **Codex wake path.** The mechanism is fixed (`codex queue`); what is
  open is its latency class, decided by whether Codex submits a queued
  message on an idle session, and whether the `stop` hook can block.
  Measured in phase 3; §10 is written to be true in every outcome.
- **Codex plugin manifest capabilities.** If `.codex-plugin/plugin.json`
  can declare MCP servers and hooks, the setup skill goes away. Checked at
  the start of phase 2 against the installed Codex.
- **Codex marketplace format.** `codex plugin marketplace add` exists; the
  on-disk marketplace manifest format is confirmed in phase 2 before
  `.codex-plugin/marketplace.json` is written.
- **Pi adapter.** Everything in §13 marked unverified.
- **Claude Code session ID in the shim.** If Claude Code exposes its session
  ID to MCP server processes, the parent-PID join in §11 becomes a
  fallback only.
- **Clip fidelity.** DOM-to-canvas rendering misses some CSS (backdrop
  filters, some SVG). If clips are frequently wrong, a headless-Chrome
  option behind a flag is the fallback; it is not in v1.
- **Agent identity in replies.** Replies show `Agent · via <harness>`. If
  multiple sessions watch one artifact, the shell may need to show which
  session replied; the data model records it, the UI does not yet.
- **`sample()` cost control.** A per-artifact daily cap in `config.toml`
  is probably wanted before LAN mode plus a configured key is used with
  others; not in phase 5 unless asked.
