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
| D14 | Pi adapter is specified against the published extension API and clash-pi, and verified against Pi 0.73.1 in phase 2 (§13) | Pi was not installed when this was designed. |

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
- **Shim** (`artifax mcp --agent <claude|codex>`). A stdio MCP server the
  harness spawns per session. On start it ensures the daemon is up,
  registers a session record, and proxies every tool call to the daemon
  over HTTP with the token. It appends undelivered feedback to its successful
  tool results (§10 tier 1). It exits with the harness.
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
  artifax-hooks/                   per-harness hook protocol adapters (Claude Code, Codex JSON shapes)
  artifax-cli/                     binary `artifax`: serve, mcp, hook, publish, list, open, ..., doctor
web/
  shell/                           Preact + TypeScript: gallery, artifact shell, comment sidebar
  bridge/                          vanilla TypeScript: window.claude.use, comment mode, clips
  contract/                        the .d.ts files pages are written against (copied from claude.ai 0.2.61)
  dist/                            built assets, embedded into artifax-server via rust-embed
plugins/
  claude-code/                     .claude-plugin/plugin.json, .mcp.json, hooks/, skills/, commands/, scripts/
  artifax/                         Codex plugin: .codex-plugin/plugin.json, .mcp.json, hooks/, skills/, scripts/
                                   (named artifax because Codex marketplace entries point at ./plugins/<plugin-name>)
  pi/                              npm package @empathic/artifax-pi: src/artifax.ts (the extension),
                                   skills/ (the artifax skill)
.claude-plugin/marketplace.json    Claude Code marketplace pointing at plugins/claude-code
.agents/plugins/marketplace.json   Codex marketplace pointing at plugins/artifax
docs/superpowers/specs/            this document
docs/superpowers/plans/            one plan per phase
docs/contract.md                   the tool contract, sessions, page contract, and security model,
                                   for agents and humans
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
  has_clip, created_at, resolved_at, resolved_by)`; `has_clip` records
  whether `clips/<thread_id>.png` was stored. `resolved_by` is
  `viewer:<public_id>`, `viewer:anonymous` (no cookie), or
  `agent:<harness>`; it never holds a viewer cookie or a session ID, since
  thread views and events are unauthenticated.
- `comments(id, thread_id, author_kind, author_name, via_session_id, body,
  created_at)` with `author_kind` in `viewer | agent`. `via_session_id` is
  stored and never served: comment views carry `via_harness` (the replying
  session's harness, `null` on viewer comments) instead.
- `feedback(id, thread_id, comment_id, target_session_id, created_at,
  delivered_at, delivery_tier, acknowledged_at, resend_count, last_sent_at,
  untargeted_at, push_failed_at)`; one row per (comment, target session).
  `target_session_id` is null when no live session was found at send time
  or the target ended before the row was delivered (`untargeted_at` records
  when). `last_sent_at` and `resend_count` drive resends (§10).
  `push_failed_at` marks a row `codex queue` failed to take (non-zero exit,
  timeout, or spawn failure), which is then left to the in-band tiers. `delivery_tier` is one
  of `piggyback | stop_hook | prompt_hook | wait | queue | inject`.
  `acknowledged_at` is set when the target session reads, replies to, or
  resolves the thread; delivered but unacknowledged rows are resent (§10).
- `docs(artifact_id, path, json, version, updated_at)` for the `db`
  capability; `path` is the full document path such as `tasks/t1`.
- `viewers(id, public_id, display_name, created_at)` for comment authors and
  the `user` capability, keyed by the `artifax_viewer` cookie (`id`, a
  ULID). The cookie is the viewer's credential and never appears in a
  response body, event, thread view, comment, or log. `public_id` (`u_` and
  22 lowercase hex digits from 11 random bytes, unique, assigned on
  creation) is how the viewer is named to anyone else.
- `session_env(session_id, codex_home, push_error, push_error_at)`:
  per-session environment the daemon needs to push to a harness;
  `codex_home` is the `CODEX_HOME` the Codex `session_start` hook reported;
  `push_error` and `push_error_at` hold the latest `codex queue` failure
  (cleared by a success).

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
- Sessions: `POST /api/sessions` (W, register), `POST /api/sessions/join`
  (W, hooks), `PATCH /api/sessions/<id>` (W, heartbeat or end),
  `GET /api/sessions` (token), `GET /api/sessions/<id>` (token; `{session,
  push}`, where `push` is `{tier, available, reason}` plus `codex_home` for
  Codex: whether and how tier 5 reaches the session).
- Push: `GET /api/push` (no token): `{codex: {available, source, reason}}`,
  whether the daemon can push to Codex, where its `codex` came from (`env`,
  `path`, `disabled`, `not_found`), and why push is off when it is; with the
  token the object also carries `bin`, the path of the daemon's `codex`
  (`null` when it has none).
- Watches: `PUT /api/sessions/<sid>/watches/<aid>` (W), `DELETE` same (W),
  `GET /api/sessions/<sid>/watches`.
- Comments: `GET /api/artifacts/<aid>/threads` (`include_resolved`,
  `cursor`, `limit`), `GET .../threads/<tid>`, `GET .../threads/<tid>/clip`
  (the PNG; `Content-Security-Policy: sandbox`, `nosniff`), `POST
  .../threads` (create thread with first comment; no token), `POST
  .../threads/<tid>/comments` (viewer: no token; agent: W,
  `author_kind=agent`, and `X-Artifax-Session` naming a live session), `POST
  .../threads/<tid>/send` (no token; sets `sent_to_agent`, creates feedback
  rows), `POST .../threads/<tid>/resolve` (viewer, or agent with W and
  `X-Artifax-Session`). Thread views carry `clip_path` only for requests with
  the token.
- Viewers: `GET /api/viewers/me` (creates the viewer and sets the
  `artifax_viewer` cookie on first contact), `PUT /api/viewers/me`
  (`{display_name}`; empty clears it); both answer `{viewer: {public_id,
  display_name, created_at}}` and never echo the cookie. No token. The viewer routes (thread
  creation, comments, send, resolve, and these two) refuse a foreign `Origin`
  (§14).
- Feedback: `GET /api/sessions/<sid>/feedback?wait=<secs>&tier=<tier>&resends=<true|false>`
  (W; long-poll, returns undelivered feedback for that session and marks it
  delivered by the named tier when the response is produced; `resends`,
  default true, also includes resend-eligible rows for the tiers that resend,
  `piggyback` and `stop_hook`), `POST /api/sessions/<sid>/feedback/ack`
  (W; `{thread_ids?, comment_ids?}`: acknowledges every row on the named
  threads, and only the named comments' rows, so a comment the caller has not
  seen stays pending).
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
  "title": "Quarterly Review",          // required on version 1 (the tools take it from <title> when omitted), optional after
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
  viewer public ID.
- **downloads**: `save({filename, data})` triggers a browser download after
  a shell confirmation.
- **user**: `id()`, `me()`, `isOwner()`, `canEdit()`, `can(name)`,
  `profiles(ids)`, `search(q)`. Identity is the viewer cookie, exposed to
  pages only as the viewer's `public_id` (`id()` returns it; the cookie
  never reaches a page); names come from the `viewers` table; `guest` is
  `false` always.
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
   mirroring claude.ai. Agent replies and resolves need a live session
   (`X-Artifax-Session`; 400 `unknown_session` without one), so they fail
   through the sessionless `/mcp`. Resolving a thread, by the viewer or the
   agent, withdraws its feedback rows that have not been delivered.

### Feedback payload

Rendered as text so it can be dropped into any harness:

```
[artifax] Comment sent to you on "Quarterly Review" (http://localhost:7480/a/7q3k9mzx2b4t), thread 01J9...
Anchored on: main > section:nth-of-type(2) > h2  «Quarterly goals»  (v3)
Clip: /Users/alex/.artifax/artifacts/7q3k9mzx2b4t/clips/01J9....png
Alex: "Make this a two-column layout and drop the third bullet."
Reply with comments_reply, then comments_resolve when done.
```

### Delivery tiers (D8)

| Tier | Mechanism | Harnesses | Latency | Failure modes |
|---|---|---|---|---|
| 1 | Undelivered feedback is appended to every successful tool result of a session-bound shim or Pi extension (errors and the sessionless `/mcp` carry none; `wait_for_feedback`'s own result is tier 4) | all three | next tool call | Nothing arrives while the agent is idle or not using artifax tools. |
| 2 | Stop hook: if undelivered feedback exists for a watched artifact, output "block" with the payload as reason | Claude Code and Codex (both verified: `{"decision":"block","reason":...}` on stdout with exit 0 continues the turn with the reason as input and the hook fires again with `stop_hook_active: true`) | end of the current turn | Only fires when a turn ends; an idle session is not woken. Loop guard: a feedback row is delivered once, and the hook allows the stop when nothing new exists, honouring `stop_hook_active`. |
| 3 | Prompt-submit hook adds pending feedback as additional context; the `SessionStart` hook also adds what is pending when a session starts | Claude Code (`UserPromptSubmit` and `SessionStart`); Codex (`SessionStart` only) | the user's next message | Depends on the user typing something. |
| 4 | `wait_for_feedback` tool: long-polls the daemon for up to `timeout_s` | all three | immediate while waiting | Harness tool timeouts cap a single call (Codex defaults to 60 s), so the tool defaults to 50 s and returns "nothing yet, call again"; the skill tells the agent to loop while the user wants live feedback. |
| 5 | Native push | Codex: `codex queue --thread <id> --message` (below). Pi: the extension API's `sendUserMessage`, which starts a turn when idle (§13). Claude Code: none available to third-party plugins. | Codex: under a second when the session's TUI is idle (measured 0.17 s), the end of the running turn when it is busy, never while no TUI is attached; Pi: at once when idle, after the current turn when streaming | See below, and the resend rule. Tier 5 (Codex queue and Pi inject) is skipped while the target session is inside a `wait_for_feedback` call; tier 4 delivers instead. Only `wait_for_feedback` polls (`tier=wait`) count; a Pi `inject` poll made meanwhile takes nothing and answers empty at once. |

### Delivery and acknowledgement

A feedback row is marked `delivered_at` by the first tier that hands it
to the harness. Delivery is not proof the agent saw it: a queued Codex
message or an injected Pi message can be dropped by the harness. So the
row also carries `acknowledged_at`, set when that session calls
`comments_read`, `comments_reply`, or `comments_resolve` on the thread, or
when tier 1 or tier 4 returns it (those paths are in-band and count as
seen). Rows delivered by tier 2, 3, or 5 and unacknowledged after 2 minutes
are included again by tier 1 on the next tool result and by tier 2 on the
next stop (not while `stop_hook_active` is set), marked `(resent)`. Resends
stop after acknowledgement or after three attempts; the thread then shows
"delivered, not acknowledged" in the shell.

### The Codex wake path, stated plainly

Mechanism: when a feedback row targets a Codex session whose
`harness_session_id` is known and whose watch has `replies_armed`, the
daemon runs

```
codex queue --thread <harness_session_id> --message <payload>
```

with the payload from above, a 10 s timeout, and the daemon's environment
plus the `CODEX_HOME` the Codex `session_start` hook recorded for that
session (the daemon's own environment lacks it; without a recorded value
the default `~/.codex` applies). The daemon claims the rows (delivered, tier
`queue`) before running the command, so no other tier hands them over
meanwhile, and publishes the claimed `feedback_state` at once; exit 0 keeps
the claim. It means "queued", not "seen". The thread
ID is the `session_id` the Codex `session_start` hook received, joined to the
shim's session record by parent PID (§11).

Measured on Codex 0.158.0 (2026-09-29): the CLI sends the message to the
shared Codex app-server daemon (starting it if needed; `--no-daemon` is
refused), which persists it in `$CODEX_HOME/queue_1.sqlite`. Outcomes:

- The session's TUI is idle at its prompt: the message is submitted with no
  keystroke about 0.17 s later. This is a true wake.
- The TUI is mid-turn: the message is submitted when that turn ends (2 ms
  after `task_complete` in the measurement), with no keystroke.
- No TUI is attached (the TUI exited, or the thread belongs to
  `codex exec`): the message is held durably, `codex queue` still exits 0,
  and it is drained on the next `codex resume <id>`. For such sessions tier
  5 never wakes anything; tiers 1, 2 and 4 apply.

`docs/contract.md` records the same result.

Failure modes and what happens in each:

- **Thread ID unknown.** The Codex hooks did not run (not installed, or
  Codex has not granted persisted hook trust), so no `session_start` hook
  registered the ID. The daemon skips tier 5 for that session, the `status`
  tool reports "Codex session ID unknown, native push disabled", and
  `artifax doctor --agent codex` explains how to install and trust the
  hooks. Tiers 1 and 4 still apply (tier 2 needs the hooks that did not
  run).
- **`codex queue` fails.** A non-zero exit never means the session exited
  (the measurement never produced one for an exited session, which is held
  instead; a non-zero exit is a missing app-server, a malformed thread ID, a
  logged-out CLI, or another CLI failure), so it is treated like a timeout
  (10 s) or a failure to start `codex`: the daemon releases the claimed rows
  and marks them `push_failed_at`, so they wait on tiers 1 to 4 for the same
  session and are not queued again until they are retargeted to another
  session. The session stays live with its watches, and the failure is
  recorded on its push state (`push.last_error`, such as `codex queue exited
  with code 1`, and `push.last_error_at` in `GET /api/sessions/<id>` and the
  `status` tool; a later successful run clears both). Dispatch never ends a
  session.
- **`codex` not on the daemon's PATH.** The daemon was started by a process
  with a minimal environment. Tier 5 is disabled for all Codex sessions and
  `doctor` reports it. The shim passes its own `PATH` when it auto-starts
  the daemon, which covers the common case.
- **Queued but never surfaced.** Exit 0 but the agent never sees it. The
  acknowledgement and resend rule above catches it: the row is resent
  in-band on the next tool result or stop, up to three times.
- **Stop hook.** The Codex `Stop` hook runs `artifax hook --agent codex
  stop`. Codex honours `{"decision":"block","reason":...}` (and exit 2 with
  the reason on stderr): the turn continues with the reason as a user-role
  `<hook_prompt>` and the hook fires again with `stop_hook_active: true`
  (verified on 0.158.0). Hooks from a project's `.codex/hooks.json` are
  ignored until the project is trusted; the plugin's `hooks/hooks.json` is
  the delivery path.

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
exits and the daemon notices the closed connection. When a session ends, each
of its undelivered feedback rows is untargeted, or deleted when another live
session already targets the same comment; untargeted rows go to the next
session that publishes a version of or watches the artifact.
`replies_armed` mirrors claude.ai's auto-reply arming and gates tier 2 and
tier 5; tiers 1, 3 and 4 work for every target session (the artifact's owner
session, watching or not, and every watcher).

## 11. Sessions and identity

The shim registers a session on start:

```json
{"harness":"claude","harness_session_id":null,"cwd":"/Users/alex/proj",
 "pid":48213,"parent_pid":48200}
```

Claude Code passes `CLAUDE_CODE_SESSION_ID` (with `CLAUDE_PID` and
`CLAUDE_PROJECT_DIR`) to MCP servers, so under Claude Code the shim registers
with the harness session ID and the hook joins by that ID. Codex does not: it
passes MCP servers only `PATH`, `PWD` and the variables the plugin's
`env_vars` lists, so the Codex shim registers without one. The hooks do
receive it. `artifax hook --agent <x> session-start` therefore registers
`{harness, harness_session_id, cwd, parent_pid, ancestor_pids}` and the
daemon joins the two records on `(harness, parent_pid)`, trying the hook's
ancestors nearest first when its parent is a wrapper shell (Codex runs hooks
under `bash`), since both the shim and the hook descend from the same harness
process. Where a harness exposes
the session ID in the shim's environment, the shim sends it and the join is
by ID instead. Claude Code additionally sets `CLAUDE_CODE_SESSION_ID` for
hook processes, as toolpath's plugin relies on.

The registered `cwd` is `CLAUDE_PROJECT_DIR` (else the shim's own working
directory) under Claude Code. Pi has no shim: its extension registers
`{harness: "pi", harness_session_id, cwd, pid, parent_pid}` itself (§13).
Codex starts the shim in the plugin's directory, so under Codex the shim
registers its parent process's working directory (the Codex session's), read
from `/proc/<ppid>/cwd` on Linux and `lsof` on macOS, or an empty `cwd` when
that fails; the `session-start` hook then fills an empty one.

Heartbeats: the shim `PATCH`es `last_seen_at` every 60 s; a session with no
heartbeat for 5 minutes and a dead PID is marked ended.

## 12. MCP tool surface

Exposed by the shim (stdio) and the daemon (HTTP). Names are shared across
harnesses; each harness prefixes them: Claude Code shows them as
`mcp__plugin_artifax_artifax__<name>` when installed as a plugin and as
`mcp__artifax__<name>` from a plain `.mcp.json` entry named `artifax`, Codex
as `mcp__artifax__<name>`, and the Pi extension registers them as
`artifax_<name>`.

Artifacts: `publish` (file_path or html, files map, title, description,
icon, capabilities, url to update, if_version, label; creating an artifact
needs a title, which the tool takes from the page's first `<title>` when
`title` is omitted and refuses with `invalid_args` when there is neither),
`read` (url or id, optional path), `list` (limit, scope mine|all, files
scope), `delete`, `open` (opens the browser on the machine running the tool
and reports `opened` from the opener's exit status within 1.5 s), `pin`,
`unpin`, `asset_upload` (file_path or file_paths).

Comments: `comments_read` (url_or_id, thread_id, cursor, include_resolved),
`comments_reply` (url_or_id, thread_id, text), `comments_resolve`
(url_or_id, thread_id), `watch` (url_or_id, on, replies), `wait_for_feedback`
(url_or_id optional, timeout_s default 50, raised to at least 1 and capped
at 600). The artifact argument keeps the phase 2 name `url_or_id`.

Data: `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`,
`db_str_replace`, `db_batch`, with `collection`, `doc_id`, `data`,
`file_path`, `if_version`, `as_level` as on claude.ai.

Server: `status` (daemon URL, version, this session's ID and watches, and
`push`: whether tier 5 reaches this session and why not).

Every tool result is JSON text plus, when present, a trailing
`---\n[artifax] N comments sent to you:\n...` block (`1 comment` when N is
1; tier 1). URLs in
results are always the daemon's browser URL so they can be pasted to a
person.

The CLI exposes the same operations as `artifax <cmd> --json` for harnesses
without MCP and for scripts, including `artifax read` and `artifax asset
upload`, whose `--json` output is the `read` and `asset_upload` tool's result
object. The other commands print their own JSON shape.

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

### Codex (`plugins/artifax`)

The directory is named after the plugin because a Codex marketplace entry
must point at `./plugins/<plugin-name>`.

- `.codex-plugin/plugin.json`: name `artifax`, version, `skills: "./skills/"`,
  `mcpServers: "./.mcp.json"`, and the `interface` block (display name,
  descriptions, developer, category, capabilities, three `defaultPrompt`
  strings). The manifest has no `hooks` key: Codex rejects it there and
  discovers `hooks/hooks.json` by convention.
- `.mcp.json`: `{"mcpServers": {"artifax": {"command": "bash", "args":
  ["./scripts/ensure-artifax.sh", "exec", "mcp", "--agent", "codex"], "cwd":
  "./", "env_vars": [...]}}}`. Codex expands no plugin-root variable in
  `.mcp.json` but resolves a relative `cwd` against the installed plugin
  root. It starts MCP servers with a minimal environment, so `env_vars`
  forwards `ARTIFAX_HOME`, `ARTIFAX_NO_OPEN`, `ARTIFAX_BIN`,
  `ARTIFAX_INSTALL_DIR`, `ARTIFAX_CONFIG_DIR`, `ARTIFAX_RELEASE_BASE_URL`,
  `ARTIFAX_RELEASE_VERSION`, and `ARTIFAX_CODEX_BIN` (which a daemon the
  shim starts inherits, §10).
- `hooks/hooks.json` in Claude Code's format: `SessionStart` →
  `bash "${PLUGIN_ROOT}/scripts/ensure-artifax.sh" exec hook --agent codex
  session-start` (joins the session, records `CODEX_HOME`, and prints the
  daemon URL and any pending `prompt_hook` feedback as context; Codex has no
  `UserPromptSubmit` hook wired), `Stop` → the same with `stop` (tier 2, timeout 10 s; the
  hook gives up after 8 s), `SessionEnd` → the same with `session-end` (Codex
  caps `SessionEnd` at 3 s, so `session-end` gives up after 2.5 s). Hooks run
  in a shell with `PLUGIN_ROOT` exported.
- `skills/artifax/SKILL.md`: same content as the Claude skill, with Codex
  tool naming (`mcp__artifax__<tool>`).
- `scripts/ensure-artifax.sh`: a copy of the Claude plugin's installer.
- Root `.agents/plugins/marketplace.json` lists it. Install:
  `codex plugin marketplace add <repo>` then `codex plugin add artifax@artifax`.

Person-side settings, documented in the plugin README rather than set by the
plugin:

- Hooks run only with `features.hooks = true` and after the person trusts
  them (Codex asks in an interactive session; `codex exec` skips untrusted
  hooks). Without hooks the tools work and the session is registered by the
  shim alone.
- Codex asks before each MCP tool call, and `codex exec` (approval policy
  `never`) refuses such calls. `[plugins."artifax@artifax".mcp_servers.artifax]
  default_tools_approval_mode = "approve"` approves every artifax tool,
  including `delete`.

### Pi (`plugins/pi`)

- npm package `@empathic/artifax-pi` with `pi.extensions: ["src/artifax.ts"]`
  and `pi.skills: ["skills"]` (Pi reads only the resources a `pi` manifest
  lists once one exists). Installed from a clone with
  `pi install /absolute/path/to/artifax/plugins/pi`, or loaded for one run
  with `pi -e <path>`. It needs the `artifax` CLI on `PATH` or `ARTIFAX_BIN`.
- `src/artifax.ts` registers `artifax_<tool>` for the fourteen tools through
  `registerTool`, with TypeBox schemas mirroring `tools.rs` and results
  identical to the MCP tools. The package carries the Artifax version, so
  `status` compares the daemon's version with the Artifax version and
  reports `daemon_version` only on real skew. The tools call the daemon's
  REST API over
  `node:http`, finding the daemon through `daemon.json` and starting it with
  `artifax serve` when none is running. A tool error is thrown, so Pi marks the
  result `isError`, with the JSON error body intact as its text.
- On `session_start` the extension registers
  `{harness: "pi", harness_session_id: <Pi session ID>, cwd, pid, parent_pid}`
  within a 3 s budget (the first tool call registers when that does not
  finish); on `session_shutdown` it ends the session with a 3 s deadline. It
  sends no heartbeat: a row left behind when Pi exits uncleanly lapses through
  the daemon's reaper. A tool call whose session was ended under it
  (`unknown_session`) registers a new session and retries once, as the shim
  does.
- The extension appends undelivered feedback to its own successful tool
  results (tier 1), as the shim does, from a `tool_result` handler. For tier
  5 it long-polls `GET /api/sessions/<sid>/feedback?tier=inject` (50 s per
  poll; a 5 s pause after a failed poll or one that came back empty within a
  second; it finds a running daemon but never starts one) from registration
  to `session_shutdown`, and hands each payload to
  `pi.sendUserMessage(text, {deliverAs: "followUp"})`.
- The `/artifax open [id] | list | status` command.
- `skills/artifax/SKILL.md`: the same skill as the other plugins, with
  `artifax_<tool>` names, relative file paths resolved against the Pi session's
  working directory, and the `/artifax` command.

Verified against `@mariozechner/pi-coding-agent` 0.73.1:

1. Events: `session_start`, `session_shutdown`, `tool_call` and `tool_result`
   exist. `tool_call` and `tool_result` carry `toolName` and `input` (and
   `toolCallId`); no event carries `sessionId`, which comes from the context
   (item 5).
2. Tools: `registerTool` takes a TypeBox parameter schema and an async
   `execute` returning `{content, details}`.
3. MCP: an extension cannot register an MCP server, hence the direct REST
   calls.
4. Message injection: `sendUserMessage` exists and starts a turn when the
   agent is idle, so tier 5 is possible for Pi in phase 3.
5. Session ID: read with `ctx.sessionManager.getSessionId()`; Pi does not put
   it in spawned processes' environment.
6. Install: `pi install <path>` (or `npm:<pkg>`); `package.json`'s
   `pi.extensions` is read.
7. UI: `ctx.hasUI`, `ctx.ui.notify` and `ctx.ui.select(title, options)`
   exist.

## 14. Security model

- Default bind `127.0.0.1`. `artifax serve --bind 0.0.0.0` or
  `config.toml` opts into LAN. The gallery header shows the LAN URL when
  bound.
- Write endpoints, and the session reads (`GET /api/sessions` and
  `GET /api/sessions/<id>`, whose rows carry working directories and process
  IDs), need the bearer token from `daemon.json` (0600). Local
  shims, hooks, and the CLI read it; the shell on localhost fetches it from
  `/api/token`, which only answers to loopback connections and also requires
  a literal local `Host` header (localhost, 127.0.0.1, [::1]).
- Every `/api` route (token or not) refuses DNS rebinding: the `Host`
  header must be `localhost`, `127.0.0.1` or `[::1]` (with or without the
  port), or exactly the address and port the connection arrived on (the
  bind address; for an unspecified bind, the interface address the client
  reached, as an IP literal). Anything else, any other DNS name included, is
  403 `forbidden_host`. Artifact hosts (`<aid>.localhost`) never reach
  `/api`; the shell, content, and `/healthz` are not checked. LAN viewers can
  view, comment, send to agent, resolve, and write `db` docs at
  `interact` level; they cannot publish, delete, upload assets, or write
  `admin`-level docs.
- The viewer routes (creating a thread, commenting, sending to the agent,
  resolving, `GET`/`PUT /api/viewers/me`) refuse a request whose `Origin` is
  not the daemon's own (`http://` plus the request's `Host`; artifact
  origins, `null`, and other origins are refused) with 403
  `forbidden_origin`, so a published page cannot comment, send, or resolve
  on the person's behalf. Requests without an `Origin` header are allowed.
  The daemon is HTTP only. `GET /api/push` needs no token but names the
  daemon's `codex` path (which usually contains the user name) only to
  requests with the token.
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
- Hook timeouts: every hook exits 0, silently, when its budget runs out:
  SessionStart 4 s, SessionEnd 2.5 s (Codex kills it at 3 s), Stop 8 s
  (inside the plugins' 10 s hook timeout), prompt submit 4 s. Each daemon
  call inside uses a 3 s timeout (2 s in SessionEnd).
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
  structure checks and Codex's plugin validator for the Codex plugin, and a
  manual Codex smoke test that installs it into a scratch `CODEX_HOME`; the Pi extension through a fake
  `ExtensionAPI` object against a real daemon, plus a manual `pi -p` smoke
  test (`scripts/smoke-pi.sh`).
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
commands, Codex plugin, Pi extension, release
workflow producing binaries with checksums. Ship when: an agent in Claude
Code and Codex can publish and update an artifact through MCP, and the Pi
extension passes its tests against a real daemon.

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
- **Phase 3 cookie scoping.** The viewer cookie is host-only, `HttpOnly`,
  and validated as a ULID by the daemon (the shell never reads it), since
  `<aid>.localhost` shares a site with `localhost`.
- **Codex wake path.** Resolved by measurement on 2026-09-29 (§10): a
  queued message is submitted at once to an idle attached TUI, at the end of
  the turn to a busy one, and held for a session with no TUI; the `Stop`
  hook can block. Open: whether a later Codex delivers a held message to a
  re-attached `codex exec` thread.
- **Codex plugin manifest capabilities.** Resolved in phase 2: the manifest
  declares the MCP server (`mcpServers`) and hooks are discovered from
  `hooks/hooks.json`, so there is no setup skill (§13).
- **Codex marketplace format.** Resolved in phase 2:
  `.agents/plugins/marketplace.json` (§4, §13).
- **Pi adapter.** Resolved in phase 2: the extension calls the REST API
  directly and each §13 item is verified against Pi 0.73.1.
- **Claude Code session ID in the shim.** Resolved in phase 2: Claude Code
  passes `CLAUDE_CODE_SESSION_ID` to MCP servers, so the parent-PID join in
  §11 is primary for Codex and a fallback for Claude Code.
- **Clip fidelity.** DOM-to-canvas rendering misses some CSS (backdrop
  filters, some SVG). If clips are frequently wrong, a headless-Chrome
  option behind a flag is the fallback; it is not in v1.
- **Agent identity in replies.** Replies show `Agent · via <harness>`. If
  multiple sessions watch one artifact, the shell may need to show which
  session replied; the data model records it, the UI does not yet.
- **`sample()` cost control.** A per-artifact daily cap in `config.toml`
  is probably wanted before LAN mode plus a configured key is used with
  others; not in phase 5 unless asked.
