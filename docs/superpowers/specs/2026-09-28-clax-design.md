# Clax: local Artifacts with comment-driven development for coding agents

Date: 2026-09-28
Status: draft for review

<!-- name-history:begin -->
**Name history.** Clax was named Artifax until 2026-09-29, when it was
renamed with a clean break (D15): no command, crate, package, plugin, skill,
variable, route, message prefix, header, cookie or file keeps the old name or
answers to it, and Clax never reads, migrates or deletes `~/.artifax`. The
plans in `docs/superpowers/plans/2026-09-28-*.md` predate the rename and use
the old name.
<!-- name-history:end -->

## 1. Purpose

Clax replicates the Claude Artifacts experience on a developer's own
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
  hit **Send to <agent>** (the button names the agent it sends to, for
  example **Send to claude**), and the publishing agent receives the
  selector, the quoted text, the comment, and a PNG clip of the region, then
  replies and resolves the thread from inside its session.
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
| D1 | Rust workspace, one `clax` binary, web UI embedded in the binary | Matches toolpath and clash; single install, no Node at runtime. |
| D2 | Shared per-user daemon plus a per-session stdio MCP shim (Approach A) | Stable URLs and one gallery outlive sessions; the shim gives each session an identity. |
| D3 | Daemon speaks HTTP MCP too | Harnesses that prefer HTTP MCP can skip the shim later at no cost. |
| D4 | Storage: SQLite for metadata, comments, db docs, sessions; page content and assets as files on disk under `~/.clax/` | Content is large and versioned; metadata needs queries and transactions. |
| D5 | Per-artifact origin via `<id>.localhost:<port>` when the shell is opened on localhost; opaque-origin sandboxed iframe when opened over LAN | Matches claude.ai's per-artifact origin isolation where the browser supports it; the fallback still satisfies the page contract (storage may be unavailable). |
| D6 | Runtime bridge is a script the daemon prepends to the page at serve time; it talks to the shell over `postMessage`, the shell talks to the daemon | Origin-agnostic, so D5's two modes share one code path. |
| D7 | Screenshot clips are rendered inside the content frame with a DOM-to-canvas library, not headless Chrome | No browser binary dependency; the clip is what the commenter actually saw. |
| D8 | Feedback delivery is tiered: tool-result piggyback, Stop hook re-engage, prompt-submit context, blocking wait tool, native push where a harness has one | No single mechanism works everywhere; the tiers degrade gracefully and are stated honestly in §10. |
| D9 | `watch` is armed automatically on publish for the publishing session, and only sent-to-agent comments ever reach an agent | Mirrors claude.ai; plain comments are for humans. |
| D10 | `sample()` calls the Anthropic Messages API with a configured key; provider is a trait | Simplest path to parity; other providers plug in later. |
| D11 | Write endpoints require a bearer token stored in `~/.clax/daemon.json` (mode 0600); read and comment endpoints do not | LAN viewers can view and comment; only local processes can publish, delete, or administer. |
| D12 | MCP tool names mirror claude.ai's tools (`publish`, `read`, `list`, `comments_read`, `db_get`, ...) | Agents that already know the Artifact tools transfer that knowledge; skill files carry the contract. |
| D13 | Five phases, each shippable; phase 1 has no comments, no capabilities | Per the standing instruction; comments and capabilities are the churn-prone parts. |
| D14 | Pi adapter is specified against the published extension API and clash-pi, and verified against Pi 0.73.1 in phase 2 (§13) | Pi was not installed when this was designed. |
| D15 | The product is Clax: binary `clax`, crates `clax-*`, home `~/.clax`, variables `CLAX_*`, message prefix `clax:`, routes `/_clax/`, plugin and skill `clax`; renamed from its first name with a clean break (no aliases, no migration; see the name-history note); `clax init` and `clax uninit` do remove the harnesses' registrations of the first name's plugin and marketplace, which are harness settings, not Clax data | One name everywhere; nothing was released under the first name, so there is nothing to carry over. |
| D16 | The plugins run the `clax` on `PATH` (or `$CLAX_BIN`) through a thin wrapper and never download or build; `just install` installs `clax` from the checkout and `clax init` registers the plugins embedded in the binary with each harness; `just dev <harness>` runs a fresh build from a temporary directory on `PATH`, on `~/.clax-dev` and port 7481, with the plugin loaded from the checkout for Claude Code and Pi (Codex runs its installed plugin; `just install` updates it); releases and `install.sh` serve people without a checkout | Local use never depends on a public repository or a release; a registered plugin always matches the installed binary; a moved checkout breaks nothing. |
| D17 | Grok Build is a fourth harness, served by a separate plugin, `clax-grok`, whose MCP server is named `clax_grok`; it gets tiers 1, 2, 4 and 5, tier 5 being notices from `clax feedback follow` under Grok's agent-started `monitor` tool, which wake the session and deliver nothing. Grok also loads the Claude Code plugin, so in a Grok session only `--agent grok` acts: the Claude Code copy stands down (its hooks exit 0 silently; its MCP server offers one `status` tool naming clax-grok), detected by `GROK_HOOK_EVENT` for hooks and by `GROK_SESSION_ID` with a `CLAUDE_PID` that is not the server's parent for MCP | Grok discovers Claude Code plugins and keeps only the first MCP server of a name, so a shared plugin or server name would let one copy shadow the other; deciding by harness rather than by start order gives one acting copy in every combination. |
| D18 | Claude Code's tier 5 is a notice, not a delivery: a Claude Code channel event from the shim when the session was launched with the channel (`--dangerously-load-development-channels plugin:clax@<marketplace>`), else a background `clax feedback follow --once` that the skill starts after a publish. The shim never declares permission relay | Channels are the only way a third-party plugin can start a turn in an idle Claude Code session, but they are a research preview behind a launch flag. The background command works in every session. A notice that points at a comment cannot double-deliver it. Commenters must never approve tool use |
| D-Echo | The shell's look is Echo, and the agent working signal, the version changelog and batch send are built in it, as recorded in `2026-10-01-echo-design.md`: comment threads with version-tagged history and no turns; gallery cards without thumbnails (Q1); a theme switch that returns to the system (Q2); @mentions by whole display name (Q3); looking at a thread clears it, while the Addressed group holds still until the view is decided again (Q4); presence kept in memory, with the location from the selected thread (Q5); keys only while focus is in the shell (Q6); each person's last viewed version public, their per-thread marks private (Q7); agents named by harness, with no publishing state (Q8); rally of 10 at v10 only (Q9); any viewer may resolve (Q10); the returning viewer's summary in the top bar (Q11); the version bands moved into the top bar (Q12); a send reaching only the agent it names (§10). | The artifact stays the star: nothing covers or moves it, people and agents keep fixed colours and places, and every screen shows the threads from the viewer's own point of view. |

## 3. Architecture

```
                 ┌──────────────────────────────────────────────────────────┐
                 │  clax serve  (one per user, auto-started, port 7480)     │
   browser ──────┤  HTTP: shell UI, gallery, content, assets, REST API      │
   (shell +      │  SSE/WS: live events (versions, comments, room)          │
    content      │  MCP over HTTP (D3)                                      │
    frame)       │  SQLite + files under ~/.clax/                           │
                 └───────▲──────────────▲──────────────▲────────────────────┘
                         │ HTTP+token   │ HTTP+token   │ HTTP+token
        ┌────────────────┴───┐  ┌───────┴────────┐  ┌──┴──────────────────┐
        │ clax mcp           │  │ clax hook      │  │ clax <cli cmd>      │
        │ (stdio MCP shim,   │  │ (SessionStart, │  │ (publish, open,     │
        │  one per session)  │  │  Stop, Prompt, │  │  comments, doctor)  │
        └────────▲───────────┘  │  SessionEnd)   │  └─────────────────────┘
                 │ stdio        └───────▲────────┘
        ┌────────┴────────────────────────┴────────┐
        │ harness: Claude Code | Codex | Grok | Pi │
        └──────────────────────────────────────────┘
```

Components:

- **Daemon** (`clax serve`). Owns storage, serves the browser, emits
  events, hosts MCP over HTTP, runs `sample()` calls. Started on demand by
  any CLI or shim invocation that finds no live daemon; stays up until
  `clax stop` or reboot.
- **Shim** (`clax mcp --agent <claude|codex|grok>`). A stdio MCP server the
  harness spawns per session. On start it ensures the daemon is up,
  registers a session record, and proxies every tool call to the daemon
  over HTTP with the token. It appends undelivered feedback to its successful
  tool results (§10 tier 1). It exits with the harness.
- **Hooks** (`clax hook --agent <x> <event>`). Short-lived processes the
  harness runs at lifecycle points. They read the harness's JSON on stdin,
  call the daemon, and print the harness's expected JSON.
- **Shell UI**. The page at `/a/<id>`: top bar, version menu, comment mode
  toggle, thread sidebar, people panel, and an iframe holding the content.
  Also the gallery at `/`.
- **Content + bridge**. The published HTML wrapped in the document skeleton
  with `/_clax/bridge.js` prepended. The bridge implements
  `window.claude.use`, comment-mode hit testing and highlighting, anchor
  resolution, and screenshot clips.
- **Plugins**. One directory per harness that packages the shim command,
  hooks, skills, and slash commands, plus a thin wrapper that runs the
  `clax` on `PATH` and explains when there is none (§13, D16).

## 4. Repository layout

```
Cargo.toml                         workspace, edition 2024
crates/
  clax-core/                       types, IDs, storage (SQLite + files), anchors, events
  clax-server/                     axum HTTP/SSE server, REST API, MCP-over-HTTP, sample provider
  clax-mcp/                        stdio MCP shim (rmcp), session registration, feedback piggyback
  clax-hooks/                      per-harness hook protocol adapters (Claude Code, Codex JSON shapes)
  clax-cli/                        binary `clax`: serve, mcp, hook, publish, list, open, ..., doctor
web/
  shell/                           Svelte 5 (runes) + TypeScript, Vite SPA: gallery, artifact shell, comment sidebar
  bridge/                          vanilla TypeScript: window.claude.use, comment mode, clips
  contract/                        the .d.ts files pages are written against (copied from claude.ai 0.2.61)
  dist/                            built assets, embedded into clax-server via rust-embed
plugins/
  claude-code/                     .claude-plugin/plugin.json, .mcp.json, hooks/, skills/, commands/, scripts/
  clax/                            Codex plugin: .codex-plugin/plugin.json, .mcp.json, hooks/, skills/, scripts/
                                   (named clax because Codex marketplace entries point at ./plugins/<plugin-name>)
  clax-grok/                       Grok Build plugin: .grok-plugin/plugin.json, .mcp.json, hooks/, skills/, scripts/
  pi/                              npm package @empathic/clax-pi: src/clax.ts (the extension),
                                   skills/ (the clax skill)
.claude-plugin/marketplace.json    Claude Code marketplace pointing at plugins/claude-code
.agents/plugins/marketplace.json   Codex marketplace pointing at plugins/clax
.grok-plugin/marketplace.json      Grok Build marketplace index pointing at plugins/clax-grok
docs/superpowers/specs/            this document
docs/superpowers/plans/            one plan per phase
docs/contract.md                   the tool contract, sessions, page contract, and security model,
                                   for agents and humans
justfile, scripts/quality_gates.sh same gate style as toolpath
scripts/ensure-clax.sh             the plugins' wrapper (copied into each MCP plugin's scripts/)
scripts/dev.sh, watch.sh           `just dev <harness>` and `just watch`
scripts/*release*, check-version.sh, bump-version.sh
                                   release packaging and version checks (.github/workflows/release.yml)
install.sh                         installs a release into ~/.local/bin, for people without a checkout
```

Dependency graph:

```
clax-cli ── clax-server ── clax-core
        ├── clax-mcp    ── clax-core
        └── clax-hooks  ── clax-core
```

Key crates: `axum`, `tokio`, `tower-http`, `rusqlite` (bundled),
`rust-embed`, `rmcp` (official Rust MCP SDK), `serde`, `ulid`, `reqwest`
(shim, hooks, and the sample provider), `clap`. Web: Svelte 5, Vite,
TypeScript, `modern-screenshot` for clips. No CSS framework.

## 5. Storage and data model

Root: `~/.clax/` (override with `CLAX_HOME`).

```
~/.clax/
  daemon.json            {port, pid, token, started_at, bind, version, exe}  mode 0600
  clax.db                SQLite
  artifacts/<aid>/
    versions/<n>/index.html          the page as published (before wrapping)
    versions/<n>/files/<path>        supporting files for that version
    assets/<asset_id>.<ext>          asset store, shared across versions
    clips/<thread_id>.png            comment screenshot clips
  config.toml            [serve] port (a daemon started for this home listens there; default 7480);
                         [sample]: provider ("anthropic", or "stub" for tests and demos),
                         api_key_env (default ANTHROPIC_API_KEY), base_url (default
                         https://api.anthropic.com), max_tokens (16000), daily_call_cap
                         (none), stub_images and stub_delay_ms (stub only), and
                         [sample.models] quick|default|complex (claude-haiku-4-5,
                         claude-sonnet-5-5, claude-opus-5-5). An invalid [sample]
                         turns sample off and never stops the daemon; a file that
                         does not parse stops `clax serve`.
  marketplace/           the plugins embedded in the binary, written and registered by `clax init`
  logs/daemon.log
```

IDs: artifact IDs are 12-character lowercase Crockford base32 from 60
random bits, so URLs read `/a/7q3k9mzx2b4t`. Thread, comment, asset, and
session IDs are ULIDs. Versions are integers starting at 1.

SQLite tables (abridged; columns beyond keys are illustrative):

- `artifacts(id, title, description, icon, created_at, updated_at,
  current_version, owner_session_id, pinned, capabilities_json,
  contract_version, deleted_at)`
- `versions(artifact_id, n, label, created_at, session_id, files_json, note)`
  where `files_json` maps published path to content type and size; `note` is
  the agent's short change note for the version (at most 280 characters, null
  when none).
- `assets(id, artifact_id, content_type, ext, size, created_at)`
- `sessions(id, agent_handle, harness, harness_session_id, cwd, pid,
  parent_pid, started_at, last_seen_at, ended_at)`; `agent_handle` (`a_` and
  22 lowercase hex digits from 11 random bytes, unique, assigned at
  registration, never derived from the ID) names the session's agent to the
  shell, which never sees a session ID.
- `watches(session_id, artifact_id, replies_armed, created_at)`
- `threads(id, artifact_id, version_n, anchor_json, status, sent_to_agent,
  target_session_id, has_clip, created_at, resolved_at, resolved_by)`;
  `has_clip` records whether `clips/<thread_id>.png` was stored.
  `target_session_id` is the session the thread was last sent to with `to`
  (null when it was last sent without one; §10 "Data flow"); it is stored and
  never served. `resolved_by` is
  `viewer:<public_id>`, `viewer:anonymous` (no cookie), or
  `agent:<harness>`; it never holds a viewer cookie or a session ID, since
  thread views and events are unauthenticated.
- `comments(id, thread_id, author_kind, author_name, author_public_id,
  via_session_id, body, created_at)` with `author_kind` in `viewer | agent`.
  `via_session_id` is stored and never served: comment views carry
  `via_harness` (the replying session's harness, `null` on viewer comments)
  instead; `author_public_id` is the writing viewer's `public_id` (null for
  agents, viewers without a cookie, and comments written before it existed).
- `feedback(id, thread_id, comment_id, target_session_id, batch_id,
  created_at, delivered_at, delivery_tier, acknowledged_at, resend_count,
  last_sent_at, untargeted_at, push_failed_at, notified_at)`; one row per
  (comment, target session); `batch_id` names the batch send that created the
  row, if any.
  `target_session_id` is null when no live session was found at send time
  or the target ended before the row was delivered (`untargeted_at` records
  when). `last_sent_at` and `resend_count` drive resends (§10).
  `notified_at` records when `clax feedback follow` announced the row to its
  target (a notice, §10 "Notices"); it is cleared when the row is retargeted.
  `push_failed_at` marks a row `codex queue` failed to take (non-zero exit,
  timeout, or spawn failure), which is then left to the in-band tiers. `delivery_tier` is one
  of `piggyback | stop_hook | prompt_hook | wait | queue | inject`.
  `acknowledged_at` is set when the target session reads, replies to, or
  resolves the thread; delivered but unacknowledged rows are resent (§10).
- `docs(artifact_id, path, collection, json, version, updated_at)` for the
  `db` capability; `path` is the full document path such as `tasks/t1` and
  `collection` its parent path, indexed for queries.
- `leases(artifact_id, path, holder, expires_at)` for the `db` single-writer
  lease (`acquire({holder})`, 30 s TTL).
- `viewers(id, public_id, display_name, created_at)` for comment authors and
  the `user` capability, keyed by the `clax_viewer` cookie (`id`, a
  ULID). The cookie is the viewer's credential and never appears in a
  response body, event, thread view, comment, or log. `public_id` (`u_` and
  22 lowercase hex digits from 11 random bytes, unique, assigned on
  creation) is how the viewer is named to anyone else.
- `session_env(session_id, codex_home, push_error, push_error_at)`:
  per-session environment the daemon needs to push to a harness;
  `codex_home` is the `CODEX_HOME` the Codex `session_start` hook reported;
  `push_error` and `push_error_at` hold the latest `codex queue` failure
  (cleared by a success).
- `version_threads(artifact_id, version_n, thread_id, source, created_at)`:
  the threads a version addressed (§10, "Version changelog"); `source`
  is `working`, `explicit` or `resolve`. Deleting a thread deletes its links.
  Linking never changes a thread's status.
- `viewer_seen(viewer_id, artifact_id, seen_n, updated_at)`: the latest
  version this viewer (the `clax_viewer` cookie's row) has viewed at the
  artifact's latest URL (`/a/<aid>`, not a `/v/<n>` URL).
  It only moves forward; at most 200 rows per viewer (the least recently
  updated are pruned on write); deleting an artifact deletes its rows.
- `send_batches(id, artifact_id, note, sent_by, size, created_at)` and
  `batch_threads(batch_id, thread_id)`: batch sends to the agent (§10,
  "Batch send"); `sent_by` is the sender's display name as a comment author
  gets it, never a cookie. Deleting a thread deletes its `batch_threads`
  rows (the batch keeps its `size` as sent); deleting the artifact deletes
  its batches and their `batch_threads` rows.
- `viewer_threads(viewer_id, thread_id, looked_at)`: when this viewer last
  looked at the thread (§10, "Participants and attention"). Served only to
  that viewer. Deleting a thread deletes its rows.
- `mentions(comment_id, public_id)`: the viewers a comment @mentions.

Working records (§10, "Working") and presence (§10, "Presence") are not
stored: the daemon keeps them in memory, so a restart starts with none.

Content addressing: every version keeps its own files; files omitted from a
later publish are copied forward as on claude.ai, `null` removes one. A
version is never rewritten.

## 6. HTTP API

All routes are on the daemon. Write routes (marked W) require
`Authorization: Bearer <token>`.

Browser-facing:

- `GET /` gallery shell. `GET /a/<aid>` artifact shell. `GET /a/<aid>/v/<n>`
  shell pinned to a version. Either may be followed by `/<path>`, a
  published page the frame opens on (`GET /a/<aid>/<path>`,
  `GET /a/<aid>/v/<n>/<path>`); every `/a/<aid>/...` path serves the shell
  (see §8 for how it reads the path).
- `GET /` → `index.html`; `GET /a/<id>[/v/<n>][/<file>]` → `artifact.html` with
  the bootstrap block and, when the frame mode is known, the content `<iframe>`
  (§8 Time to usable). Both are HTML responses with an `ETag` over the exact
  bytes sent: `/` is `Cache-Control: no-cache`; `/a/…`, which carries one
  viewer's data, is `private, no-cache` with `Vary: Cookie`. The bootstrap never
  holds the daemon token and is escaped for a `<script>` element (`<`, `>`,
  `&`, U+2028 and U+2029 as `\u` escapes).
- `GET /c/<aid>/v/<n>/` wrapped content document (bridge prepended).
  `GET /c/<aid>/v/<n>/<path>` supporting files; a file stored as
  `text/html` is wrapped exactly like the index (the bridge tag carries
  `data-file="<path>"`, `index.html` for the index; cached per artifact,
  version, and file), every other file is served as stored. Also served at
  `http://<aid>.localhost:<port>/v/<n>/...` for D5. On `<aid>.localhost` the
  daemon serves `/v/...`, `/healthz`, `/_clax/*`, and `/_blob/*` and
  404s everything else.
- `GET /_blob/<asset_id>` asset bytes.
- `GET /_clax/bridge.js`, `/_clax/shell/*` static.
- `GET /_clax/bridge/<part>-<hash>.js`: the bridge's lazy parts, ES modules
  with content-hashed names, `Cache-Control: public, max-age=31536000,
  immutable` and `Access-Control-Allow-Origin: *` (a sandboxed frame imports
  them from an opaque origin; a missing part's 404 carries it too). The eager
  `bridge.js` names them, so its `?v=` hash changes whenever a part does. No
  path under `/_clax/` with a component starting with `.` is served.

Browser caching (every route above):

- Every HTML response (the shell's own document on every `/` and `/a/...`
  path, and every wrapped page, `/c/...` and `/v/...` on an artifact host
  alike, including a `text/html` file that is not UTF-8 and so is served as
  stored) carries `Cache-Control: no-cache` and an `ETag`: the browser may
  store it but revalidates it on every load, and the daemon answers
  `304 Not Modified` when the `If-None-Match` it sends is current. No HTML
  response is ever `immutable` or given a long `max-age`, so a page, and the
  bridge tag inside it, is never older than the daemon serving it.
- Bridge tags name the bridge by version:
  `/_clax/bridge.js?v=<first 12 hex digits of the bundle's SHA-256>`,
  computed once when a release daemon starts (a debug build reads the
  bundle from `web/dist` on every request, so it rehashes whenever the
  file's modification time or size changes and the wrap cache drops pages
  naming an older version). In a release build that URL is served
  `public, max-age=31536000, immutable`; the bare `/_clax/bridge.js`, a
  `?v=` naming another bundle, and every bridge URL of a debug build are
  served `no-cache` (with an `ETag`). A bridge tag is recognised only in the
  exact form the daemon writes (`<script src="/_clax/bridge.js"` or
  `...bridge.js?v=<hex>"`, then ` data-artifact="`), so a page's own string
  or comment containing the URL is kept.
- Non-HTML supporting files (`/c/<aid>/v/<n>/<path>`) and `/_blob/<asset_id>`
  are `public, max-age=31536000, immutable`: their URLs name one version's
  or one asset's bytes, which never change. The shell's own bundles under
  `/_clax/shell/` have content-hashed names and are immutable too in a
  release build (`no-cache` with an `ETag` in a debug build).
- A `200` JSON answer to an API `GET` carries an `ETag` of its bytes and
  `Cache-Control: no-cache` unless the route sets its own; a matching
  `If-None-Match` gets `304`. API responses and the shell's pages, bundles
  and bridge are compressed (`br` or `gzip`, by `Accept-Encoding`) when the
  body is text of 1 KiB or more; event streams, images and fonts never are.
- `GET /api/events?artifact=<aid>` SSE stream: `version`, `thread`,
  `comment`, `thread_resolved`, `thread_deleted`, `feedback_state`, `doc` events (rooms use their own WebSocket, not SSE).
  `artifact_deleted` is sent when an
  artifact is deleted, with the artifact's ID in its data. `resync`, with
  `data: {"dropped": n}`, is sent when a subscriber fell behind and `n` events
  were dropped; the client should refetch the state it displays.
- The event stream also carries `working` (`{artifact_id, working: [view]}`,
  the artifact's whole list after any change, §10 "Working"). An optional
  `types=<name>,<name>` narrows the stream to those event names (`ready` and
  `resync` are always sent); the gallery opens `?types=working` with no
  `artifact` filter.
- `GET /api/stream` is the multiplexed form: one SSE stream per client,
  whose topics change over `POST /api/stream/<stream>` (`{"subscribe":
  [...], "unsubscribe": [...]}`, answered `{seq, topics}`) without
  reopening it. Topics are `gallery` (versions, thread summaries without
  bodies, deletions and working summaries without messages, for every
  artifact), `artifact:<aid>` (its versions, threads as deltas and feedback
  states), `presence:<aid>` (changed people and the public IDs gone),
  `working:<aid>` (its whole working list) and `docs:<aid>` (document
  events at the caller's level). The caller (token in `Authorization` or as
  the events cookie, the viewer cookie) is fixed when the stream opens, and
  each subscription is checked once, when made, with the `db` routes'
  levels. Every event names its topic and carries `id: <stream>:<seq>`,
  one daemon-wide sequence.
  Fan-out: one channel per subscribed topic, each keeping its last 64
  events; per stream, a queue of 64. A stream whose queue fills gets
  `resync` (`{topic, reason: "behind"}`) for that topic, its queued events
  of the topic dropped. A dropped connection's stream is held 60 s; a
  reconnect from the same caller with `Last-Event-ID: <stream>:<seq>`
  resumes it with the events it missed, or `resync` (`reason: "gap"`) for a
  topic whose 64 kept events no longer reach back. The full protocol is in
  `docs/contract.md` "Event stream protocol"; `/api/events` is served
  unchanged beside it, for agents and other clients.
- The shell uses `/api/stream` only, through one connection per browser: a
  SharedWorker holds it for every Clax tab of the origin, subscribes it to
  the union of the tabs' topics, routes each event to the tabs that watch
  its topic, and drops a topic when its last tab lets it go (where
  SharedWorker is missing, tabs elect a leader with Web Locks and a
  BroadcastChannel; without those, one stream per tab). Views subscribe and
  unsubscribe as they mount and unmount; they refetch only when their
  topics go live or get `resync`, and apply deltas otherwise. A tab hidden
  for 30 s leaves the connection (a leader hands it on) and joins again
  when it shows; the
  connection closes when no tab watches anything. The connection
  authenticates with the events cookie (`Path=/api/stream`, as for
  `/api/events`), never a token in a URL. The number of connections does
  not grow with tabs, and each client costs the daemon a bounded queue and
  one entry per topic.

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
  .../threads` (create thread with first comment; no token; `via_page=true`
  marks a comment the page wrote through the `comments` capability, whose
  `@agent` then sends nothing), `POST
  .../threads/<tid>/comments` (viewer: no token, optional `via_page` as
  above; agent: W,
  `author_kind=agent`, and `X-Clax-Session` naming a live session), `POST
  .../threads/<tid>/send` (no token; sets `sent_to_agent`, creates feedback
  rows). `POST .../threads:send` (no token; foreign `Origin` refused, as the
  single send) takes `{thread_ids, note?, to?}` (`note` at most 280
  characters after whitespace is collapsed, else `note_too_long`) and sends 1
  to 20 threads as one batch, all or nothing. It answers `{batch, sent, unchanged, threads}`,
  or 400 `invalid_args` / `note_too_long` / `unknown_agent` /
  `unknown_thread` / `thread_resolved`, or 409 `nothing_to_send`, and writes
  nothing on any error. Thread views carry `sends`, the batches that sent
  them. The single send takes an optional JSON body `{to}` (an agent handle;
  400 `unknown_agent` when it names no live agent on the artifact). `POST .../threads/<tid>/resolve` (viewer, or agent with W and
  `X-Clax-Session`), `POST .../threads/<tid>/reopen` and `DELETE
  .../threads/<tid>` (viewer at `interact` or above, that is a named viewer
  or the owner shell with the token, else 403 `forbidden`; or the agent with
  `{"as": "agent"}` / `?as=agent`, W, and `X-Clax-Session`, on sent
  threads only, else 200 `{guidance}`; reopen answers `{thread}`, delete
  `{deleted: true, thread_id}` and removes the comments, feedback rows, and
  clip). Thread views carry `clip_path` only for requests with the token.
- Viewers: `GET /api/viewers/me` (creates the viewer and sets the
  `clax_viewer` cookie on first contact), `PUT /api/viewers/me`
  (`{display_name}`; empty clears it); both answer `{viewer: {public_id,
  display_name, created_at}}` and never echo the cookie. No token. The viewer routes (thread
  creation, comments, send, resolve, reopen, delete, and these two) refuse a foreign `Origin`
  (§14).
- Feedback: `GET /api/sessions/<sid>/feedback?wait=<secs>&tier=<tier>&resends=<true|false>`
  (W; long-poll, returns undelivered feedback for that session and marks it
  delivered by the named tier when the response is produced; `resends`,
  default true, also includes resend-eligible rows for the tiers that resend,
  `piggyback` and `stop_hook`), `POST /api/sessions/<sid>/feedback/ack`
  (W; `{thread_ids?, comment_ids?}`: acknowledges every row on the named
  threads, and only the named comments' rows, so a comment the caller has not
  seen stays pending).
- Notices: `GET /api/sessions/<sid>/notices?wait=<secs>` (W; long-poll,
  capped at 600 s; returns `{notices: [{feedback_id, comment_id,
  thread_id, artifact_id, title, url}], lines, waited_s}` for the
  session's armed rows that no tier has delivered and no follower has
  announced, and sets their `notified_at`; delivers nothing; answers
  empty at once while the session is in `wait_for_feedback`; 404 for an
  unknown session, 400 `unknown_session` for an ended one).
- Working (§10, "Working"): `GET /api/artifacts/<aid>/working` (no token;
  `{working: [view]}`), `GET /api/sessions/<sid>/working` (token; views plus
  `session_id`, `artifact_id`, `expires_at`), `PUT
  /api/sessions/<sid>/working/<aid>` (W; `{thread_ids?, message?}`; `{working,
  message_truncated}`), `DELETE /api/sessions/<sid>/working/<aid>` (W;
  optional `?thread_ids=<id>,<id>`; `{cleared, working}`), `POST
  /api/sessions/<sid>/working/renew` (W; `{renewed}`), `POST
  /api/sessions/<sid>/working/end` (W, turn end; `{cleared}`). A view is
  `{key, harness, message, thread_ids, started_at, last_heartbeat}`; `key` is
  a ULID minted for the record, never a session ID. `GET /api/artifacts` and
  `GET /api/artifacts/<aid>` carry each artifact's `working` list.
- Changelog (§10, "Version changelog"): version views carry `note` and
  `addresses` (thread IDs); thread views carry `addressed_in` (version
  numbers). `GET /api/viewers/me/seen?artifact=<aid>` (`{seen}`) and `PUT
  /api/viewers/me/seen` (`{artifact_id, version}`, monotonic; `{seen}`) are
  viewer routes (no token, foreign `Origin` refused; PUT without a viewer
  cookie is 400 `no_viewer`).
- Participants and attention (§10, "Participants and attention"): `GET
  /api/artifacts/<aid>` and the bootstrap block carry `participants`
  (`{people: [{public_id, display_name, seen}], agents: [{handle, harness,
  live}]}`; `seen` is the latest version that person has viewed, public by
  design, §10)
  and, when the request's viewer cookie names a viewer, `attention`
  (`{addressed, addressed_v, new_replies, open_in, seen, looked}`); `GET /api/artifacts`
  carries `participants` per artifact. `GET /api/viewers/me/attention`
  answers `{artifacts: {<aid>: {addressed, addressed_v, new_replies, open_in, seen}}}`
  for every artifact (`{artifacts: {}}` without a cookie). `PUT
  /api/viewers/me/looked` takes `{artifact_id, thread_ids}` (at most 50) and
  answers `{looked: {<thread ID>: <time>}}`. Both are viewer routes; every
  response carrying `attention` is `private, no-cache` with `Vary: Cookie`.
- Presence (§10, "Presence"): `PUT /api/viewers/me/presence` takes
  `{artifact_id, state: "here" | "away", where?}` (viewer route; a cookie is
  required) and answers `{people}`; `GET /api/artifacts/<aid>/presence`
  answers `{people: [{public_id, display_name, state, where, since}]}`; the
  event stream carries `presence` with the same body.
- Docs (db capability): `GET/PUT/PATCH/DELETE /api/artifacts/<aid>/docs/<path>`,
  `GET /api/artifacts/<aid>/docs?collection=<c>&where=...&order_by=...&limit=&cursor=`,
  `POST /api/artifacts/<aid>/docs:batch` (at most 50 operations, atomic),
  `POST /api/artifacts/<aid>/docs:str_replace`, `POST
  /api/artifacts/<aid>/docs:acquire`. Callers holding the token (agents, the
  CLI, the `db_*` tools) must send `if_version` when writing an existing
  document; page-side writes through the bridge are marked `lww` and are
  last-writer-wins, as claude.ai's `db.d.ts` promises. A page's requests
  carry `X-Clax-Via: page`. SSE `doc` events carry the path and version
  only, and reach a subscriber only when its level may read that path
  (private `data/users/<id>/` subtrees reach their owner alone). Access
  rules from the artifact's declared `db.rules` are evaluated per caller
  level (§9). A caller without the token is refused 403 `not_declared`
  when the artifact's current declaration does not include `db` (a publish
  or a metadata edit can drop it). An ordered query without `limit` returns
  every match, or fails `resource_exhausted` past 32 MiB of document
  bodies. Threads also gain the reopen and delete routes above
  (`thread_deleted` SSE event) for the `comments` capability, and
  `GET /api/viewers?ids=|q=` for `user.profiles()`/`search()`; a version
  created by a page's `artifact.publish` carries `by_page: true`.
- Sample: `GET /api/artifacts/<aid>/sample` (SameOrigin; availability,
  limits, `calls_today` and `daily_call_cap`, with `available: false` and
  `provider: null` to a caller without the token); `POST
  /api/artifacts/<aid>/sample` (SameOrigin, the token and a viewer cookie)
  streams `start`, `text`, `tool_call`, and one `done` or `error` over SSE;
  `POST /api/artifacts/<aid>/sample/<call_id>/tool_result` (SameOrigin, the
  token and a viewer cookie) returns a page tool's result for the round
  (204); `GET /api/sample` (token) reports the provider, why sample is off,
  the key's variable name and the daily cap, for `clax doctor`. Without the
  token a call is 401 `unauthorized`, without a cookie 403 `forbidden`.
  Closing the stream drops the provider request at once.
- Room: `GET /api/artifacts/<aid>/room?peer=<label>[&token=<bearer>]`
  upgrades to a WebSocket, opened by the shell in both frame modes (frames
  never reach `/api`). `<label>` is 16 characters of `[0-9a-z]`, one per open
  document, reused on reconnect; a newer socket for a label closes the older
  one 4409 `replaced`. Frames are JSON with a `t` field; the full protocol is
  in `docs/contract.md` "Room protocol".
- Streams that cannot carry the bearer header (`/api/events`, the room
  WebSocket) accept it as a `?token=` query parameter, which is never
  logged; a valid token with a viewer cookie makes the subscriber `admin`
  (the owner shell), a valid token without a cookie `owner`, a cookie alone
  `interact` when named and `view` otherwise.

Health: `GET /healthz` returns `{version, pid, started_at}` on every Host,
including `<aid>.localhost`; CLI, shim, and the D5 probe use it.

### Publish body

`POST /api/artifacts` and `POST /api/artifacts/<aid>/versions` take JSON:

```json
{
  "title": "Quarterly Review",          // required on version 1 (the tools take it from <title> when omitted), optional after
  "description": "…", "icon": "chart",  // optional
  "label": "Draft to legal",            // optional, ≤ 60 chars
  "note": "Two columns; third bullet dropped",  // optional, ≤ 280 chars, longer is cut
  "addresses": ["01J9..."],                    // optional, ≤ 50 threads of this artifact
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

A thread in `addresses` that is not a thread of the artifact is 400
`unknown_thread`, and nothing is published.

`index.html` is required on every publish and is never carried forward.
Other files carry forward from the previous version unless given or
`null`. `content_type` defaults from the extension. The 64 MB cap is on
decoded bytes (the HTTP body limit is 96 MiB to cover base64 inflation) and
a single file is capped at 16 MB, as on claude.ai. Assets use multipart at
`POST /api/artifacts/<aid>/assets`.

A file path is relative: one or more non-empty segments separated by `/`,
no `.` or `..` segment, no backslash, and no control characters or U+2028 /
U+2029 line and paragraph separators (a path appears in anchor summaries and
payload lines, which must stay one line); anything else is `invalid_path`.

## 7. Daemon discovery and lifecycle

1. A client reads `~/.clax/daemon.json`, calls `/healthz` on that port,
   and checks the PID is alive. Match: use it.
2. Otherwise it takes an exclusive `flock` on `~/.clax/daemon.lock`,
   re-checks, then spawns `clax serve --daemonize` detached (new session,
   stdio to `logs/daemon.log`), and polls `/healthz` for up to 5 seconds. The
   spawning client holds the lock until `/healthz` answers.
3. `clax serve` binds `127.0.0.1` on `--port`, else the home's `[serve] port`, else 7480, tries the next 20
   ports if busy, and writes `daemon.json` atomically. `clax serve` itself
   does not take the lock.
4. `clax stop` sends `POST /api/admin/shutdown` (W). The daemon also
   exits if `daemon.json` is replaced by a newer daemon (checked every 30 s)
   and exits when `daemon.json` is missing on two consecutive checks, so a
   stale process cannot shadow a new one.
5. Version skew: `daemon.json` records the daemon's `version` and `exe`
   (its executable's canonical path). The MCP shim and `clax serve`, when
   either finds a daemon older than itself, replace it; other CLI commands
   and the Pi extension use whatever daemon answers. A newer daemon, or one
   of the same version, is kept (a newer daemon serves older clients, and
   two plugins at different versions must not restart each other's daemon),
   so `just install` stops the agents' daemon itself when it runs the
   binary just replaced. A replacement holds
   `daemon.lock` throughout: it re-reads `daemon.json`, asks the daemon to
   shut down (SSE streams and long polls end, in-flight requests get 5 s),
   waits up to 7 s for its PID to exit, starts the new executable on the old
   port and bind address, and waits for `/healthz`. Storage migrations run
   on daemon start.

## 8. Shell UI and viewer

Gallery (`/`): two groups. **Needs your eyes** holds the artifacts where,
for this viewer, a thread they are in was addressed after they last looked
at it, a version newer than the last one they viewed exists, or a thread
they are in has a reply from someone else they have not seen. **Everything
else** follows, pinned first, then by the latest version or reply. Each card
leads with its version numeral, then the title, the publishing agent and
time, markers (`N addressed in vK`, `vK new`, `N new replies`, `<agent>
working on N`, `N open`), and a footer with the roster (people on the left,
agents on the right, at most 3 a side) and `seen vK` (the last version this
viewer viewed). Search filters by title and description; each card opens the
artifact and can be pinned or deleted. An artifact at v10 shows a muted
`rally of 10` chip. The footer holds one haiku, a new one each visit. There
are no thumbnails.

Artifact shell (`/a/<aid>`, `/a/<aid>/v/<n>`, either followed by
`/<path>` of a published page; after `/a/<aid>`, `v` followed by an
all-digit segment is a version and anything else starts the page path, so a
file under `v/<digits>/` is reachable only through the versioned form):

- URL: the frame opens on the page the URL names (`index.html` when none; a
  page the version does not hold shows a message instead of the frame). Each
  move to another page is one history entry. The bridge hands a plain click on
  a link to another page of the version to the shell (`clax:navigate`),
  which pushes that page's URL and moves the frame with `location.replace`,
  both carrying the link's fragment (the shell takes a `#` fragment of at
  most 512 characters, and at most one link per greeting page);
  the sidebar's jump to a thread on another page does the same, and
  `popstate` moves the frame to the URL's page. Any other navigation in the
  frame keeps the frame's own entry, and the shell replaces its URL when the
  new page greets with a different `file`. A hello naming a page the version
  does not hold is ignored, and a frame load with no hello since the previous
  load shows no pins. The URL fragment and the frame's are kept in step: the
  frame opens at the URL's fragment, and the bridge reports every fragment
  change (`clax:hash`), which the shell writes to its URL with
  `replaceState`. The version picker, the Reload button, and copy link keep
  the current page. `url` in tool results stays `/a/<aid>`.

- Top bar: the Echo mark (a plain link to the gallery), the title over the
  "published by" line, the roster and its two-line summary (who is working
  on what; what is new for you; opens the people panel; at an artifact's
  tenth version it reads `rally of 10` once per browser), Comment (red-orange
  when on, with a 3px red-orange rule under the bar, and the C keycap),
  Threads with the open count, the version button (`v5 of 5`, a green dot
  while a version newer than this viewer's last view exists) opening the
  version menu, a menu with open raw and copy link, and the theme switch. At
  phone width: the mark, the title, the roster (one per side), Comment and
  the menu with open raw and copy link; a Page | Threads switch sits at the
  foot.
- Version menu: a panel listing every version newest first, each with who
  published it and when, the threads it addressed as numbered chips, what
  this viewer did about them, and its note; each a link to that version
  (older versions read-only). A full sheet at phone width.
- People panel (the roster): one row per person (threads they are in,
  presence and location, and the last version they viewed, which is public:
  `participants.people[].seen`) and per agent (the threads it is working on
  and whose, elapsed time with a haiku, or idle), and the viewer's own name,
  edited here, with a "Share where I'm looking" switch (on by default,
  stored per browser) that stops reporting `where`. An agent is named by its
  harness (`claude`); when two agents on the artifact share a harness, each
  name gains the first four hex digits of its handle (`claude 7f3a`).
- Keys: C comment mode; `?` opens a sheet listing the keys; Esc leaves
  comment mode or closes a menu or the sheet. There are no others. Keys act
  only when focus is in the shell and not in a text field or a dialog, and
  C and `?` are held while the viewer may still be typing for the page
  (after the shell window loses focus, after a prompt or composer the page
  raised opens or closes, and after a reload the page's publish caused)
  until the viewer presses in the shell or focus lands on one of its
  controls.
- Consequential actions and the keyboard trail: Send, Resolve, Reply, the
  batch Send and "Send N unsent", the composer's Post when the page opened
  the composer, and the consent prompt's Allow run only on a trusted
  pointer's click (a click whose `detail` is 1 or more) once focus has
  entered the shell from the frame or from `<body>` without a press of the
  viewer's on a shell control. Every load starts that way, and Allow always
  takes a click. Any other activation (Enter, Space, ⌘↵, an assistive
  technology's) does nothing and says "Click to <verb>" beside the action.
  No key ends it: only a trusted press that puts focus on the shell control
  it targets, never one on the gesture shield or the prompt's backdrop.
- Theme: follows the system; the switch flips light and dark, and a choice
  equal to the system's clears back to following it.
- Nothing Clax draws covers or moves the artifact, except pins and the
  comment-mode outline.
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
- Live updates: the shell watches its artifact's topics on the browser's
  shared `/api/stream` connection; a new version shows
  "v4 published" in the top bar's summary with a Reload button beside it
  unless the page published it itself via the
  `artifact` capability, in which case it reloads immediately as on
  claude.ai. Nothing is drawn over the page for a version: viewing an older
  version shows a Latest link beside the version button, the version button
  gets its dot, the summary line reads `v5 addressed 3` (or `3 new versions
  · 7 addressed` for a viewer returning after several), and the Addressed
  group fills.
- Thread sidebar: groups **Addressed in vN** (open threads this viewer is
  in that the newest version addressed and they had not looked at when the
  view was decided; the view is decided when it loads and again when a new
  version arrives, so looking at a thread marks it at once but leaves it in
  the group until then), **Open** (also the threads earlier versions
  addressed, their history line naming the version, and outdated threads,
  with their tag), **Detached** (anchor not found on its own page in this
  version) and **Resolved**. A card shows the anchor summary, an
  `outdated` tag when its element changed in a later version but still
  exists (resolved by selector or quote while its `html_hash` differs), the
  clip, the messages (people's with a red-orange rule on the left, an
  agent's with a green rule on the right, `<agent> · addressed in vN` when
  linked), one line of version-tagged history (`v3 alex commented · v4 Mia
  replied · claude worked on it · v5 claude addressed it · alex resolved`),
  and Reply, Resolve and `Send to <agent> ▾`. Any viewer may resolve a
  thread, and an agent may resolve a thread sent to it; the history records
  who. Clicking a thread scrolls the frame to its anchor and
  flashes it (a static outline under reduced motion); a thread on another
  page is labelled "on <file>" and clicking it navigates there first. Pins
  show only for the page in the frame: red-orange; split red-orange and
  green while an agent works on the thread; white with a green ring and a
  `vN` flag when addressed and not looked at; a green ring when selected;
  dashed while being written. Open thread cards carry a checkbox
  (Shift-click ticks a range). While any is
  ticked, a selection bar at the top of the sidebar reads `N selected · sent
  together`, with Clear, `Send N to <agent> ▾` and an optional one-line note
  (Cmd+Enter or Ctrl+Enter sends). A `Send N unsent to
  <agent>` button sits at the sidebar top whenever open threads have not
  been sent. A sent thread's history shows the send and its note. A thread
  that disappears leaves the selection.
- Working: while an agent works on the artifact, the top bar's summary reads
  `<agent> working on N` (`claude, codex working on 5` for several;
  `<agent>: <message>` when the record has a message; `<agent> working` when
  it names no threads), the roster's agent token is solid green with a
  breathing dot, a 2px green sweep runs under the bar, each named thread
  shows `<agent> is working on it` with the elapsed time, and the sidebar
  starts with a strip naming the threads and a haiku. `<agent>` is the
  agent's name (its harness, as in the people panel). The summary is
  a polite live region that changes only when the records change.
- Comment mode: works on every HTML page of the version (each is served
  with the bridge, which greets the shell with its `file`). Over an element
  taller or wider than the viewport, or covering more than 60% of it, the
  target is the text under the pointer (its line in preformatted text, else
  its block or sentence), picked as a range anchor. Text-like inline content
  (`span`, `code`, `em`, `strong`, `b`, `i`, `mark`, `small`, `sub`, `sup`,
  `kbd`, `samp`, `var`, `abbr`, `cite`, `q`, `time`, `u`, `s`; a token of
  highlighted code) inside such an element is not a target of its own: the
  line, block, or sentence around it is. Controls and replaced elements
  (`button`, `input`, `select`, `textarea`, `a[href]`, `img`, `svg`, `video`,
  `audio`, `canvas`, `iframe`, `object`, `[role=button]`, `[contenteditable]`,
  `label`, `summary`) stay element targets, and text-like content inside one
  targets it, unless it is itself oversized (an editable article), which is
  then treated as any other oversized element. Only the text near the
  pointer is read on hover (at most 4,000 characters each way). The outline
  is clamped to the viewport with all four borders visible, a tint of at
  least 16% and a border of at least 3:1 contrast on the background behind
  the target (its nearest ancestor with an opaque background, else the
  page's colour scheme), in any CSS colour syntax. The bridge
  highlights the hovered element with an outline
  and shows a floating pin cursor. Click selects that element; drag-select
  text creates a range anchor. A drag that starts where no text is under the
  pointer (the caret there is not in visible, non-blank text within 4 px of
  the pointer, or the pointer is over `img`, `svg`, `canvas`, `video`,
  `iframe`, `object`, `embed`, or `picture`), or any drag with Shift held,
  draws a rectangle instead: shown live in the outline's colours, clamped to
  the viewport with all four borders visible; Escape drops it; one narrower
  or shorter than 8 px is a click. Releasing it creates an area anchor; the
  rectangle stays drawn, dashed, until its clip is taken, and no other pick
  starts meanwhile. While
  Option (Alt) is held the target is the enclosing element of the target
  under the pointer (a line's nearest block ancestor, an element's parent);
  each Up press widens one more ancestor (never past `body`), Down narrows
  back, releasing Option returns to the usual target, and a click picks the
  widened element; a drag selection with Option held picks the widened
  element too (at least the selection's block), and an Option press never
  starts an area drag or a native image drag. The bridge takes Option from its
  own key events and from the pointer events' `altKey`; the shell forwards
  Option, Up and Down while Option is held, and Escape, as `clax:key`
  (`key` one of `Alt`, `ArrowUp`, `ArrowDown`, `Escape`; `down` a boolean;
  anything else is ignored) while comment mode is on and the pointer is over
  the frame, unless focus is in a text field. A forwarded Escape drops a drag
  in progress, else the bridge answers `clax:cancel` and the shell leaves
  comment mode; Escape elsewhere leaves it at once. The bridge's comment mode
  acts only on trusted (viewer) events and has at most one pick in flight;
  it sends `clax:pick-start {pickId, version, anchor}` at the viewer's
  click or release, before rendering the clip, and the shell takes it only
  in comment mode and only while the frame holds the viewer's gesture
  (`frameGesture`): it opens the composer at once, focused and taking the
  screenshot, turns comment mode off, and takes the `clax:pick` with that
  ID once, for its clip only (a pending pick is forgotten when comment mode
  comes back on or a page greets); a second start with the viewer's gesture
  while one is pending and comment mode is off means one was forged, so
  both are refused and the composer closes (it stays when the viewer has
  typed in it). The bridge renders the clip only after the shell's
  `clax:composer-ready {pickId}`, sent once the composer's textarea has
  focus, and none after `clax:pick-refused {pickId}` (sent for every start
  the shell refuses) or when no answer comes within 5 s (it then posts the
  pick with no clip, so the composer stops waiting; `pick.ts`). The
  composer focuses its textarea in a layout effect, and the clip's work,
  which can hold a main thread the frame shares with the shell, never
  delays that focus; keys typed during it wait in the browser's input queue
  and reach the focused textarea. A key typed in the page before that focus
  (15–19 ms measured) goes to the page; no page text ever enters a
  composer. The bridge takes only trusted `message` events (`isTrusted`),
  so a page cannot pass a message of its own making off as the shell's. The
  bridge keeps the shell's window as it was at load, so a page replacing
  `window.parent` cannot read or alter what it posts. Page calls and picks
  that need the viewer's gesture use one of two tiers (`caps/gesture.ts`;
  the verb table is in the contract, "The viewer's gesture"). Shell input is
  recorded allow-all: every trusted event reaching the shell window of the
  types through which input can grant a document activation or marks the
  viewer's interaction (`keydown`, `mousedown`, `pointerdown`, `pointerup`,
  `touchend`, `click`, `auxclick`, `dblclick`, `contextmenu`, `drop`,
  `dragstart`, `dragend`, `pointercancel`, `wheel`, and the text events
  `beforeinput`, `input`, `compositionstart`, `compositionupdate`,
  `compositionend` and `textInput`, which an input method, the emoji picker
  or dictation dispatch without a key press); the shell window losing focus
  to anything but the content frame (read on the next tick); focus sitting
  on anything in the shell's document outside the element it renders into,
  other than the content frame, `body` and `html` (an extension's frame,
  directly or in an open or closed shadow root, whose host then holds focus;
  checked every 100 ms); and the pointer leaving such an element while the
  shell has activation. The keys the shell forwards to the
  page are the exception. `frameGesture` (the composer tier: `openComposer`,
  `compose`, picks) holds only when the viewer's latest input went to the
  frame, as the shell sees it from its own trusted events: transient user
  activation, focus in the frame, and either the pointer arrived on the
  frame after the viewer's latest shell input and is on it now, or focus
  entered after that input with none since and, at entry, the pointer had
  so arrived or a Tab or Shift+Tab pressed in the shell moved it. An
  arrival counts only by the viewer's own input over the frame: a
  `mouseover` of it at a spot other than the boundary event just before it
  (which a layout change under a resting pointer repeats) and more than
  2 px from where the pointer was at that input (the shell takes positions
  from every trusted pointer event, boundary events included); a `mousemove`
  over a band (below) with real movement, however small (non-zero
  `movementX`/`movementY` or changed screen coordinates, which layout
  re-hit-tests never send); a wheel over a band; or a touch press on a band.
  `frameGestureStrict` (every write as the viewer: `create`, `reply`,
  `resolve`, `delete`, `sendToClaude`; and `artifact.publish`) adds no shell
  input of any kind for 5.5 s. It adds no pointer rule of its own; the
  composer tier's check it includes uses the pointer. The shell's script
  starting counts as such input only when the shell is already active as it
  starts (input came before the script ran); otherwise no earlier input can
  make it active later, so a call right after a load waits for nothing.
  Every input Chromium lets grant the shell activation is one of those
  events, input to a frame outside the shell's own elements (seen as focus
  sitting there, the window's blur toward it, or the pointer leaving it),
  or input before the script ran (covered by the start rule), and the
  activation lasts 5 s, so the activation is then the frame's unless a source none of these
  see exists (none is known beyond script the viewer runs on the tab
  themselves); within that time it rejects `shell_input_recent`
  (uncharged). After shell input with a mouse over the frame (a key, text
  from an input method, a press on a shell control over it, the window
  losing focus), the shell covers the frame with transparent bands, beneath
  its controls, leaving a 9 px hole where it last saw the pointer (closed
  for 500 ms after a press on a shell control, so a double-click's second
  click within 500 ms never reaches the page): input in the hole reaches the
  page; the bands stay until the viewer's own input over them (a move with
  real movement, a wheel, a touch press), which counts as the arrival and
  lowers them (the page loses that one move or wheel event; a touch tap's
  click, hit-tested after, reaches the page and counts); a mouse press on a
  band reaches neither the page nor a shell control, is shell input, does
  not count as the pointer's arrival, leaves the bands up, and shows "Move
  the pointer, then click again" ("Move the pointer to pick" in comment
  mode). A pick refused in comment mode shows "Move the pointer to pick"
  only when it can be the viewer's own press (pointer over the frame or a
  band, focus in the frame, no arrival counted since the latest shell
  input), so a page posting starts cannot show it while the viewer uses the
  shell or while their press would count. A pick's
  composer takes focus when it opens, so the viewer types at once. This bounds forgery, it does not end
  it: a page can post a pick of its own only in comment mode, while no pick
  of the bridge's is pending, and within the browser's user-activation
  window (about five seconds) after the viewer's latest input, once the
  viewer has clicked or pressed a key in the page, has moved the pointer
  onto or over it (by a real move, not a layout change), turned the wheel or
  touched it there since their latest input to the shell, or has Tabbed
  into it. So after a click on the
  shell's Comment button, or on Cancel or Post in a composer that brings
  comment mode back, a page can forge a pick once the viewer moves the
  pointer within that window, never while it rests where that input left it
  (its script shares the bridge's window, can act then, and controls what
  gets rendered); the composer then shows the pick's quote or area label
  and its screenshot (not where it anchors), and nothing is posted without
  the viewer.
  Comment mode is off while a pick's composer is open and comes
  back on when it closes (posted, `@agent` included, cancelled, or closed
  with Escape), so the viewer comments again without pressing Comment; a
  failed post keeps the composer open and comment mode off. A composer the
  page opened (`openComposer`, `compose`), or one the viewer turned comment
  mode on and off over with the Comment button, does not turn it on when it
  closes, nor does one that closes after the artifact was deleted. Comment
  mode still ends on the Comment button and on Escape with no composer open.
  An area thread's pin sits at the area's top right; the bridge outlines the
  area dashed (from its resolved rectangle) for the thread the shell names in
  `clax:focus`: the one hovered in the sidebar or by its pin, else the
  selected one. The composer opens in the shell with the
  quote and clip preview.

Look: Echo. The interface is set in the system's sans (`ui-sans-serif,
-apple-system, "Segoe UI", system-ui, sans-serif`): titles, numerals,
labels and group heads at 600, buttons at 500, everything in sentence case.
The system's monospace (`ui-monospace, "SF Mono", Menlo, Consolas,
monospace`) sets only what is literal: code, key caps, IDs and counts. The
shell downloads no font. Greys are warm neutrals; surfaces have 8-10px
radii, hairline borders and soft shadows; buttons are quiet and fill on
hover, the primary action is solid ink and a send to an agent is green.
Red-orange is people and green is agents, in soft tints for chips and
tokens; text and controls meet WCAG AA in light and dark. Pins keep one
literal set of colours in both themes, since they sit over the artifact.
Haiku appear in the
gallery footer and under an agent's working line, never in comment mode,
never animated. Buttons are plain verbs; playful words appear only in status
lines, hints and empty states. Clicking the gallery's mark makes its halves
meet, and clicking again parts them; in the artifact view the mark is a
plain link. "Rally of 10" marks an artifact's tenth version only. The design
and its decisions are recorded in `2026-10-01-echo-design.md`.

The shell is Svelte 5 (runes mode, no SvelteKit, no SSR) + TypeScript with
CSS tokens on `:root`, dark mode via `prefers-color-scheme`, phone width
supported, so it follows the same page contract it asks of artifacts.

Phase 1 builds the gallery, the header without the comment mode toggle,
thread sidebar, and viewer name, the content frame in both origin modes,
and the version banner. The comment affordances are added in phase 3 and
must not be stubbed into phase 1.

### Time to usable

Opening a shell link must be fast. *Link → first paint* runs from the shell
document's navigation start to the artifact frame's first contentful paint.
*Link → comment ready* runs to the moment the bridge turns comment mode on in
the frame, for a viewer who presses **Comment** as soon as they can. Two more
measures isolate what the shell and the bridge control: *frame paint* is link →
first paint in a tab where nothing else happens (a press on **Comment** moves
when the frame gets to paint), and *ready latency* runs from the click on
**Comment** (the event's own timestamp) to comment mode on in the frame,
also with the comment part's bytes held back until the click (*cold ready
latency*). All five are measured in Chromium for a warm browser (the shell's files cached,
cookies set, a fresh tab), in both frame modes, by `web/perf/usable.perf.ts`,
a quality gate with budgets in `web/perf/budget.json`. Timings differ too much
between machines for one budget to judge another's, so the budgets are per
platform (keyed `<os>-<arch>`, such as `darwin-arm64`), and the gate is
enforced where the platform has budgets. On a platform without them the run
only reports its timings, under a `WARNING: … NOT GATED` line that the quality
gates print too. A platform gains budgets by recording a baseline there
(`CLAX_PERF_RECORD=baseline`); `CLAX_PERF_RECORD=budget` later lowers them.

What makes it fast:

- The shell has two entries. `/` serves `index.html` (the gallery); `/a/…`
  serves `artifact.html`, whose body already holds the page skeleton. Each
  entry loads only its own view's code. The shell's CSS is inlined in both.
- For `/a/…` the daemon injects a bootstrap block into `artifact.html`:
  `<script type="application/json" id="clax-boot">`, holding the artifact with
  its versions, every thread, and the viewer when the request's viewer cookie
  names one: what an unauthenticated browser reads from the API, less the
  session IDs (no token, no viewer cookie, no clip path, and no viewer is
  created). It follows the API's host rule (§14): a request whose `Host` the
  API would refuse gets the entry with no bootstrap and no frame. A store
  error, or a store slower than the API's request timeout, also gives the
  bare entry. The shell reads the block instead of making its first API
  calls; for a page reached through history (possibly from the browser's
  cache) it ignores the block's viewer and looks the viewer up.
- When the request carries a `clax_frame` cookie (`subdomain` or `sandbox`,
  set by the shell once it has decided the frame mode), or comes from a host
  other than `localhost` and `127.0.0.1` (always `sandbox`, as the shell has
  no artifact origins there), the daemon also injects the content
  `<iframe>` itself. The artifact then loads in parallel with the shell's
  JavaScript. The shell adopts that frame when its own decision agrees, and
  replaces it otherwise. Messages the frame posts before the shell has mounted,
  and the frame's loads, are buffered by an inline listener and replayed in
  order, through the same checks as later ones (at most 256 events; past
  that it keeps none). The same listener removes a served frame without a
  sandbox while the page is parsed when this tab has already found that
  artifact origins do not work. In a tab that has not probed yet, a served
  subdomain frame is adopted on the cookie's word, but nothing it posts is
  heard, and so no capability is answered, until the tab's probe agrees; if
  the probe disagrees, the frame is replaced unanswered.
- The bridge loads eagerly only what every page needs (`window.claude`, the
  hello, the channel, link handover, and the pick flow that renders a clip
  only once the shell's composer is ready). Comment mode with anchors and
  areas, clip rendering, and the page-side capability members are separate
  parts under `/_clax/bridge/`, loaded on need and never before the page has
  parsed (so never ahead of the page's own import maps): comment mode on the
  first shell order that needs it, and also in a task of its own once the
  page has parsed after the welcome, so it is ready when the viewer presses
  **Comment**; clip rendering once comment mode is on; capability members on
  the first `claude.use` the shell grants: `caps` for most capabilities,
  and `room` and `sample` each in a part of its own. The shell's `room` and
  `sample` handlers are chunks loaded on their first use, outside the
  artifact entry's static closure, so a page that uses neither loads
  neither. A load that fails or takes longer
  than 15 s makes the bridge post `clax:degraded`; a later need tries again
  after a backoff (2 s, doubling to 60 s), under a query naming the attempt,
  since a browser keeps a failed module load for its URL. Each part is
  self-contained (no part imports another; shared code is bundled into each),
  so a retry fetches everything that part needs. Every failed attempt is
  reported, so a page that keeps asking for a part that cannot load (calling
  `claude.use` again and again, say) brings the notice back once per backoff,
  at most once a minute. The shell remembers, per greeted
  page (forgotten whenever its gate closes: another page, version or origin,
  or a document that loaded without greeting), which parts failed and says
  so in its own words: while the comment
  part has failed, comment mode stays off and pressing **Comment** repeats
  the notice, which outranks the one for clips.

## 9. Runtime bridge and capabilities

The daemon wraps every HTML page of a version (`index.html` and every
supporting file stored as `text/html`) at serve time into the
document skeleton claude.ai uses (doctype, charset, viewport, the small
reset), inserts `<script src="/_clax/bridge.js?v=<bridge version>" data-artifact="<aid>"
data-version="<n>" data-contract="0.2.61" data-file="<path>">` as the first element of
`<head>`, then the page content. Recognition rule: if the file, after
whitespace and an optional BOM, begins with a `<!doctype` declaration
(case-insensitive), it is a complete document and is served as-is with the
bridge script inserted immediately after the doctype and any ASCII whitespace
that follows it (never before the doctype, which
would switch the page into quirks mode), so `window.claude` exists before any
page script, as `claude.d.ts` promises, whether or not the page writes a
`<head>` tag and wherever its scripts sit; otherwise it is a fragment and is
wrapped. The parser builds `<html>` and `<head>` around the bridge: a later
`<html>` tag's attributes (`lang`) are merged onto the root, and the one cost
is that a later `<head>` tag's attributes are dropped. Sub pages are placed by
the same rule. Only the first daemon bridge tag in a document runs; any other
copy stands down. Running before the page's content, the bridge defers shell
orders that read it (anchor resolution, scroll-to) until the document has
parsed. A
republished document that already carries a bridge tag (at the versioned
URL or, from before the URL carried a version, the bare one) keeps exactly
one, for the new version at the current bridge URL (§6, browser caching). This is what makes a self-republished page (which sends
the full skeleton) round-trip without nesting. Wrapping is pure and cached
per version and file. The bridge greets the shell with its page's `file`,
records it on every anchor it builds, and never resolves an anchor whose
`file` is another page.

The bridge's comment mode, clip rendering and page-side capability members
are lazy parts (§8 Time to usable); the protocol gains `clax:degraded`
(bridge → shell: `{ part: "comment" | "clip" | "caps" | "room" | "sample",
message }`, one per
failed attempt; `message` is for debugging, and the shell never shows it,
since the page could post this itself).

The bridge says `clax:bye` (bridge → shell, no fields) on `pagehide`, unless
the page is kept in the back/forward cache (`persisted`). The shell then
closes the page's gate, forgets its page, pins and failed parts, and sends
nothing in sandbox mode until a document greets; a document after it that
loads without greeting finds the gate closed. So when the frame moves to
another site's document, which the frame's sandbox lets it do, that document
gets none of the shell's messages, however long it delays its own `load`.
The bye can arrive just after that document has replaced the page (Chromium
delivers it then, with no `source`), so the shell takes it from the frame's
window or from no window, at the frame's origin; until it arrives, sends may
still reach the new document, for no longer than the old page's last message
takes to arrive. A page that posts `clax:bye` itself only cuts itself off
until it greets again, as `clax:cancel` only turns its own comment mode off;
a bye with no source could also come from another opaque-origin document
holding the shell's window (a frame of a page that opened the shell), which
can do no more than that.

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
namespace or `null`. The bridge posts `{type:"clax:use", name, id}` to
the shell; the shell answers with grant state. Unknown or undeclared names
resolve `null`. `use()` never rejects. A capability the artifact did not
declare in `capabilities` resolves `null` except `permissions` and `user`:
`user.d.ts` makes `isOwner()`, `canEdit()`, `can(name)` and `me()` work
with no declaration, so `use("user")` always resolves a namespace, and only
`id()`, `profiles()` and `search()` need the declaration.

Capability behaviour, in the same shapes as the claude.ai 0.2.61 `.d.ts`
files kept in `web/contract/`:

- **permissions** (built in): `state()` and `request()`. Grants are
  per-viewer per-artifact, stored in the shell's `localStorage`; the first
  use of a consent-gated capability shows one shell dialog. Denial is final
  for the page load.
- **artifact**: `publish(html)` sends the complete document to
  `POST /api/artifacts/<aid>/versions` with `if_version` = the version the
  frame is showing; the shell attaches the token only if the viewer is the
  owner (the shell is on localhost). A publish is the last act of the
  viewer's own interaction with the page: it needs the strict gesture tier
  (`frameGestureStrict`, §8), so without the viewer's gesture it rejects
  `rate_limited` and within 5.5 s of their input to the shell
  `shell_input_recent`, before any request; a page cannot publish on load,
  on a timer, or on the back of input to the shell. A per-tab budget also
  allows one publish per 2 s and 10 a minute. Conflict rejects
  `conflict` and the shell reloads to the winner. Read-only viewers (LAN)
  reject `not_writer`. `self` is an alias.
- **db**: `doc(path)`/`collection(path)` with get, set, update, delete,
  where, orderBy, limit, onSnapshot (over the SSE `doc` event). Rules from
  the declaration raise per-path minimums; caller level is `owner` for a
  caller holding the bearer token without a viewer (the agent, the CLI, the
  `db_*` tools; a cookie naming no viewer is no viewer), `admin` for the
  owner shell on localhost (token with a viewer's cookie; on a stream the token travels as `?token=`, see section 6),
  `interact` for a named viewer, `view` for an
  unnamed one; a viewer's level is fixed when its event stream opens. Token
  callers must pin `if_version` on existing documents; page writes are
  last-writer-wins. `acquire({holder})` is a single-writer lease with a 30 s
  TTL. `data/users/<id>/` is private per
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
  as the viewer; `resolve(id, false)` reopens and `delete(id)` deletes a
  thread through the routes in §6, which need `interact` or above. Write
  verbs ask the viewer's consent once per artifact; a page may open the
  composer 5 times in 10 s and write 10 times a minute per artifact in a
  tab. Page anchors always name the frame's page and the view's version.
  The page never learns a thread's store ID (opaque handles only; the
  write verbs act only on threads it created in its current document).
  Page-written comments are stored with `via_page` and shown and forwarded
  as written by the page; an `@agent` in them is inert. Opening the
  composer and every write as the viewer need the viewer's latest input to
  have gone to the page: opening the composer the
  composer tier (`frameGesture`); every write as the viewer (`create`,
  `reply`, `resolve`, `delete`, `sendToClaude`) the strict one
  (`frameGestureStrict`, which may reject `shell_input_recent`), §8. A
  custom-anchors page is told only the threads on its own page, as handles.
  The shell renders all threads; the page never lists them. There is no
  batch form of `sendToClaude`: a page sends threads it created one call at
  a time, each in the strict gesture tier; batches are the viewer's, from
  the sidebar. Clax extension, not part of claude.ai's contract, declared in
  `web/contract/clax-extensions.d.ts` (`ClaxExtensions.Comments`; the
  `0.2.61/` files stay claude.ai's, unchanged): `working()` resolves
  `{working, agents: [{harness, label, message, since, threads,
  otherThreads}]}`, and `onWorking(fn)` calls `fn` with that state now and
  on every change and resolves an unsubscribe function. `harness` is
  `claude`, `codex` or `pi`; `label` is its product name as people read it
  (`Claude Code`, `Codex`, `Pi`); `message` is the record's message or null;
  `since` is when the record started. `threads` holds the handles of the
  threads this document created that the agent names, and `otherThreads`
  counts the rest. Both are available under either
  declaration form (for `composer_only`, a Clax extension to that form,
  which otherwise grants only `openComposer` and `anchorFor`), need no
  consent or gesture, and never carry a store ID, session ID or record key.
- **assets**: `upload(blob)`, `list()`, `delete(id)`; owner shell only,
  `null` otherwise. Served at `/_blob/<id>`.
- **room**: `emit`, `on`, `presence`, `onPeers`, `join(name)` over one
  WebSocket per content frame's document, owned by the shell in both frame
  modes and relayed to the frame over postMessage; nothing persisted, and
  rooms are not carried on `/api/events`; bounded channels drop the oldest
  message. A topic declared `"interact"` in `capabilities.room.topics`
  admits `interact` and above, every other topic `admin` and above, and
  presence everyone. Peers are viewers only (`kind` is `"viewer"`, `guest`
  is always `false`, and no agent joins a room in v1). A `peers` snapshot
  lists at most 256 peers, the receiver among them; a subscriber that fell
  behind gets a fresh snapshot instead of the messages it missed. A version
  that stops declaring `room`, or the artifact's deletion, closes its
  sockets 4403 `revoked`. Rooms are keyed by artifact, not version: a view
  stays on its version until it reloads. The socket closes as soon as the
  frame's document leaves (its `clax:bye`, a load without a hello, or a
  shell navigation), so the peer leaves every other view at once.
- **sample** (phase 5): `sample(input, opts)` and `sample.json`, streaming
  `onText`, `tools` executed by round-tripping tool calls to the page,
  `modelTier` mapped to configured model IDs, `cache` as `sample.d.ts`
  describes (per viewer, `gcTime` up to 24 h, `refresh`, identical in-flight
  calls shared), errors `{code, message, text?}` with the `sample.d.ts`
  codes (`not_granted`, `rate_limited`, `cancelled`, `upstream_error`, …),
  partial text returned on cancellation. Only the owner's browser (the
  token and a viewer cookie) spends the key: the shell offers `sample` only
  when it holds the token, so a LAN viewer's `use("sample")` resolves
  `null`. Every call waits on consent given in that view: the first call
  asks, and the allow lasts until the tab reloads or shows another version
  and is never stored. The shell shows a running count of the artifact's
  calls today. Answers are cached per browser in the daemon, and each
  browser's calls are queued. Provider trait with an Anthropic implementation and a `stub`
  provider for tests; key from `config.toml` (`sample.api_key_env`, default
  `ANTHROPIC_API_KEY`); `[sample.models] quick|default|complex`; an optional
  `daily_call_cap` per artifact answers `rate_limited` once spent. No key
  configured: `use("sample")` resolves `null`. `config.toml` also holds
  `[serve] port`, used by `clax serve` when `--port` is absent; the bind
  address comes from `--bind` only.
- **files**, **mcp**: resolve `null`.

The bridge also handles comment mode (hit testing, outline, text selection),
anchor creation and re-resolution, and clip rendering, on messages from the
shell. The bridge never trusts messages that do not come from its shell's
window and origin, nor any the browser did not deliver itself (`isTrusted`).

### Anchors

```json
{
  "kind": "element" | "range" | "custom" | "area",
  "selector": "main > section:nth-of-type(2) > h2",
  "quote": "Quarterly goals",
  "prefix": "...", "suffix": "...",
  "html_hash": "sha256:...",
  "rect": {"x":0,"y":0,"w":0,"h":0,"scrollX":0,"scrollY":0,"viewportW":0},
  "custom_name": null,
  "file": "index.html"
}
```

`file` is the published path of the page the anchor is on: a safe relative
path of at most 512 bytes that the thread's version holds (`invalid_anchor`
otherwise); an anchor without it is on `index.html`. An anchor resolves only
on its own page. Its summary (the payload's "Anchored on" line) starts with
`<file> › ` when the file is not `index.html`.

An `area` anchor (a rectangle the viewer drew) also carries `"area": {"x",
"y", "w", "h"}`: the rectangle as fractions of the border box of the smallest
element that holds all of it (under the rectangle's centre, or an ancestor of
such an element, where an element inside inline SVG or other foreign content
counts as its outermost foreign element, the `<svg>`), each in 0 to 1 with 6
decimal places, `w` and `h` above 0, `x + w` and `y + h` at most 1. When no
element in the body holds it (below a page shorter than the viewport, or
starting in the margin of a centred body) the area is on the document:
`selector` is `html`, `html_hash` is null, and the fractions are of the whole
scrollable page (`scrollingElement`'s scroll size, from the page's top left),
unclamped. Otherwise `selector` names that element (`cssPath`, within the
1024 limit). `rect` is the rectangle in viewport pixels with the page's
scroll at draw time, and `quote` is null. The daemon refuses an area anchor without a
selector or with fractions out of range, and an `area` on any other kind
(`invalid_anchor`); `area` is absent from other kinds' JSON. Its summary is
`area in <selector> (<w>% × <h>%)` (whole percent; `<1%` for a share under
half a percent). An area re-resolves by selector (exact with a matching
`html_hash`, else selector alone; it has no quote to fall back on) and its
fractions are projected onto the element's box then; otherwise it is
detached. `area` also records a fingerprint of the element: `tag` (its local
name, cut whole at surrogate pairs), `text` (its first 32 characters of
text as a quote reads it: not in scripts or styles, CSS-hidden text
included; whitespace collapsed, control characters dropped; at most 64
characters as the daemon checks), and `children` (its child element count).
A selector-only match on a version other than the thread's (the shell sends
`sameVersion` with each anchor in `clax:resolve-anchors` and
`clax:scroll-to`) is detached when its tag differs, or when both its child
element count differs and its text (read from the shared text index) is
unlike the recorded text (a Dice similarity of character pairs under 0.5), so
live text or added rows alone keep it attached; on the thread's own version,
whose content may be live, the fingerprint is not checked. Any selector-only
match is detached when its width differs by more than 25% from its width at draw time (`rect.w /
area.w`, skipped when the viewport's width changed by more than 5% since). An
area on `html` is placed by `rect` at the same page coordinates (`x +
scrollX`, `y + scrollY`), not by its fractions.

Re-resolution order on a new version: exact `selector` with matching
`html_hash`; `selector` alone; text `quote` with `prefix`/`suffix` search;
`custom_name` for custom anchors. Nothing found: the thread is detached for
that version and stays attached to the version it was made on.

### Clips

On composer open the bridge renders a clip of the anchored region with
`modern-screenshot` whenever the region it renders fits the clip budget of
1600 × 2400 CSS px: an element that fits is rendered whole, and so is a
range's nearest block ancestor when that fits. A range inside a larger block
(a line of a whole file in one `<pre>`, a drag selection in it) is captured
as a region around the range: the lines from 120 px above it to 120 px below
it, at the block's width and at most 2400 px tall (for a range taller than
that, the 2400 px from 120 px above its start, never copying the rest of the
range), copied (with the elements between them and the block, so the page's
styles apply) next to the block off-screen, with the picked text marked by
bands of the comment-mode outline's tint, rendered, and removed; rendering
never walks the rest of the block. Before the copy is connected it is made
inert: form controls and forms lose `name` and `form` (so a copied checked
radio cannot uncheck the reader's), media, embedded content, and defined
custom elements become empty placeholders of their size, nothing keeps
`autofocus`, and the copy is `inert` and `aria-hidden`. An element larger
than the budget is rendered whole, scaled down, when it is wholly in view on
every axis over the budget; otherwise each such axis is cropped to the
element's part in the viewport at pick time, grown within the element to the
budget. No pick goes without a clip merely for its size. The PNG is at device pixel ratio, capped at 1600 px on the long side,
within a 4 s limit, in both frame modes; the bridge posts the bytes to the
shell, which uploads them with the thread. Cross-origin images that taint the
canvas are dropped from the render; the thread still stores the anchor and
quote. The clip is saved at `~/.clax/artifacts/<aid>/clips/<tid>.png` so
an agent can view it with its own file-reading tool.

Every clip is at most 5 MiB (the daemon's cap): a render over that is repeated
at half the scale, up to three times, and then given up with the reason shown
in the composer; the shell drops a pick's clip over the cap with that reason
too, and says in its notice banner when the daemon kept a thread without its
clip.

An area's clip is mandatory in intent: at release the bridge renders the
area's nearest HTML element (the body for an area on the document; the
nearest HTML ancestor of an `<svg>`, which modern-screenshot would otherwise
draw whole) cropped to exactly the rectangle,
at device pixel ratio capped at 1600 px on the long side, within a 12 s limit
(longer than other clips, since the element is often the page's main
column), in both frame modes. A failed render is reported in the composer,
which still lets the viewer post, as for other clips.

## 10. Comments and the feedback loop

### Data flow

1. Viewer enters comment mode, picks an element or selection, writes a
   comment. Shell posts the thread with anchor and clip, and comment mode
   comes back on for the next pick. Everyone with the shell open sees the
   pin via SSE.
2. Viewer presses **Send to <agent>** on the thread (or writes `@agent` in a
   comment). The daemon sets `sent_to_agent`, then creates `feedback` rows
   for the thread's viewer comments that have none yet:
   - With `to` (an agent handle naming a live owner or watcher of the
     artifact; the shell sends one whenever such an agent exists, see
     "Participants and attention"), one row per comment for that agent's
     session only, which becomes the thread's target
     (`threads.target_session_id`).
   - Without `to` (no live agent, `@agent` on a thread never sent, the
     page's `sendToClaude`), one row per comment for each live session among
     the artifact's owner session and every session with a watch on it, and
     the thread has no target. If no live session exists, the feedback is
     stored with no target and is delivered to the next session that
     publishes a version of, or watches, that artifact.
   - A later viewer comment on a sent thread (any comment, `@agent` or not)
     goes where the thread was last sent: to its target while that session
     is live; once it has ended, as a send without `to`.
3. Delivery happens by the tiers below. A feedback row is marked delivered
   once, by whichever tier delivers it first.
4. The agent calls `comments_reply` and `comments_resolve`. Replies appear
   as `Agent · via <harness>`. Only sent-to-agent threads accept agent
   replies and agent resolves; on a plain thread the tool returns guidance,
   mirroring claude.ai. Agent replies and resolves need a live session
   (`X-Clax-Session`; 400 `unknown_session` without one), so they fail
   through the sessionless `/mcp`. Resolving a thread, by the viewer or the
   agent, withdraws its feedback rows that have not been delivered.

### Feedback payload

Rendered as text so it can be dropped into any harness:

```
[clax] Comment sent to you on "Quarterly Review" (http://localhost:7480/a/7q3k9mzx2b4t), thread 01J9...
Anchored on: main > section:nth-of-type(2) > h2  «Quarterly goals»  (v3)
Clip: /Users/alex/.clax/artifacts/7q3k9mzx2b4t/clips/01J9....png
Alex: "Make this a two-column layout and drop the third bullet."
Reply with comments_reply, then comments_resolve when done.
```

A comment the page wrote through the `comments` capability, as the viewer,
is marked in the author line: `Alex (written by the page): "…"`.

### Delivery tiers (D8)

| Tier | Mechanism | Harnesses | Latency | Failure modes |
|---|---|---|---|---|
| 1 | Undelivered feedback is appended to every successful tool result of a session-bound shim or Pi extension (errors and the sessionless `/mcp` carry none; `wait_for_feedback`'s own result is tier 4) | all four | next tool call | Nothing arrives while the agent is idle or not using clax tools. |
| 2 | Stop hook: if undelivered feedback exists for a watched artifact, output "block" with the payload as reason | Claude Code, Codex and Grok Build (Claude Code and Codex verified: `{"decision":"block","reason":...}` on stdout with exit 0 continues the turn with the reason as input and the hook fires again with `stop_hook_active: true`; Grok from source: the same, with `stopHookActive`, and the hook acts only on `reason` `end_turn`) | end of the current turn | Only fires when a turn ends; an idle session is not woken. Loop guard: a feedback row is delivered once, and the hook allows the stop when nothing new exists, honouring `stop_hook_active`. |
| 3 | Prompt-submit hook adds pending feedback as additional context; the `SessionStart` hook also adds what is pending when a session starts | Claude Code (`UserPromptSubmit` and `SessionStart`); Codex (`SessionStart` only); Grok Build: none (it discards an allowing `UserPromptSubmit` hook's output and ignores `SessionStart` output) | the user's next message | Depends on the user typing something. |
| 4 | `wait_for_feedback` tool: long-polls the daemon for up to `timeout_s` | all four | immediate while waiting | Harness tool timeouts cap a single call (Codex defaults to 60 s; Grok's default is 6000 s), so the tool defaults to 50 s and returns "nothing yet, call again"; the skill tells the agent to loop while the user wants live feedback. |
| 5 | Native push | Codex: `codex queue --thread <id> --message` (below). Pi: the extension API's `sendUserMessage`, which starts a turn when idle (§13). Claude Code: a notice, through a channel (opt-in launch flag, research preview) or the follow fallback (below). Grok Build: notices from `clax feedback follow`, run by Grok's `monitor` tool, which the agent starts (below). | Codex: under a second when the session's TUI is idle (measured 0.17 s), the end of the running turn when it is busy, never while no TUI is attached; Pi: at once when idle, after the current turn when streaming; Grok: at once when idle (each line starts a turn), after the current turn when busy (from source); Claude Code: at once when idle, with the next turn when busy (both paths) | See below, and the resend rule. Tier 5 (Codex queue and Pi inject) is skipped while the target session is inside a `wait_for_feedback` call; tier 4 delivers instead. Only `wait_for_feedback` polls (`tier=wait`) count; a Pi `inject` poll made meanwhile takes nothing and answers empty at once. A Grok session is woken only once its agent has started the monitor, and never under headless `grok -p`. Claude Code: a channel that the launch flag names but policy blocks drops its events silently, and the comment then waits for tiers 1, 2 and 4. |

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

### Notices (Grok's monitor)

`clax feedback follow` long-polls `GET /api/sessions/<sid>/notices` and
prints one line per comment sent to the session, naming the artifact and
thread and saying to call `comments_read`. It never prints the comment.
The daemon announces a row only when no tier has delivered it, no
follower has announced it to this session (`notified_at` unset), and the
session watches the artifact with replies armed, and it sets
`notified_at` as it announces. A notice is not a delivery: the row still
waits for tiers 1, 2 and 4, which deliver it once under the rules above,
so a monitor never causes a second delivery. Retargeting a row clears
`notified_at`. While the session is inside `wait_for_feedback`, the
notices poll answers empty at once and announces nothing. The command
finds the session by `--session`, by `--agent` and `--harness-session`,
or by `GROK_SESSION_ID`; it never starts a daemon, follows the session
across daemon restarts, and exits 0 once the session has ended (at once
for `--session`; after 60 s with no live Clax session for a harness
session). `status`'s `push` for a Grok session is `{"tier": "monitor",
"available": <a follower polled within 15 s>, "reason": …}`.

With `--once`, `clax feedback follow` exits 0 after the first poll that
printed any line.

Claude Code receives notices in one of two ways. When the session was
launched with `--dangerously-load-development-channels
plugin:clax@<marketplace>` (or `--channels`, with an organization
allowlist entry), the shim, which declares `claude/channel`, polls the
notices route and sends each line as a `notifications/claude/channel`
event, with `meta` `{artifact_id, thread_id, comment_id}`. Otherwise the
skill has the agent run `clax feedback follow --once` in the background
after it publishes, and restart it after each exit. Claude Code wakes an
idle session when a background command exits.

Claude Code tells a channel server nothing about registration, and drops
events it does not accept. The shim polls only when its parent's command
line names a Clax channel entry. It reports `registered: null` because it
cannot know more. It never declares `claude/channel/permission`, so
nobody who comments can approve tool use.

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
  `clax doctor --agent codex` explains how to install and trust the
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
- **Stop hook.** The Codex `Stop` hook runs `clax hook --agent codex
  stop`. Codex honours `{"decision":"block","reason":...}` (and exit 2 with
  the reason on stderr): the turn continues with the reason as a user-role
  `<hook_prompt>` and the hook fires again with `stop_hook_active: true`
  (verified on 0.158.0). Hooks from a project's `.codex/hooks.json` are
  ignored until the project is trusted; the plugin's `hooks/hooks.json` is
  the delivery path.

Uniform fallback when nothing above fires: the feedback sits in the daemon
until the agent's next clax tool call or the user's next message. The
shell shows "sent, waiting for the agent" with the elapsed time and the
tier it is waiting on, so the person knows the agent has not seen it.

The same honesty applies to Claude Code: without a first-party background
task, an idle Claude Code session is woken only by tiers 2 and 3. Live
feedback on Claude Code means the agent is in a `wait_for_feedback` loop.

Grok Build has no push either, but its `monitor` tool turns each line a
command prints into a turn. Once the agent has started a monitor on
`clax feedback follow` (the skill does so after its first publish), each
comment sent to it wakes an idle session with a notice (see "Notices").
Before that, and in headless `grok -p`, comments reach it at the end of
a turn (tier 2), on its next clax tool call (tier 1), or inside a
`wait_for_feedback` loop (tier 4).

### Watch semantics

`watch` rows are created on publish for the publishing session, by the
`watch` tool, or by the `/clax:watch` command. `watch off` removes one.
A session's watches end when its `SessionEnd` hook fires or when the shim
exits and the daemon notices the closed connection. When a session ends, each
of its undelivered feedback rows is untargeted, or deleted when another live
session already targets the same comment; untargeted rows go to the next
session that publishes a version of or watches the artifact.
`replies_armed` mirrors claude.ai's auto-reply arming and gates tier 2 and
tier 5; tiers 1, 3 and 4 work for every target session. A watch makes a
session a possible target, not a recipient of every comment: a send with
`to` reaches only the named agent, and the owner and every watcher receive a
comment only when it is sent without `to`, or when the agent its thread was
sent to has ended (Data flow, step 2).

### Working

A working record says that a harness session is acting on an artifact, and
optionally on some of its threads, now. The daemon keeps records in memory
(a restart starts with none) keyed by session and artifact; each holds a
`key` (a ULID), the session's `harness`, an optional `message` (at most 140
characters; longer is cut), up to 20 open thread IDs, `started_at` and
`last_heartbeat`. A record lapses 120 s after its last heartbeat: reads stop
showing it at once, and a sweep every 5 s removes it and sends `working`.

- Marked (created, or renewed with the threads added) whenever the daemon
  hands feedback to the session by tier `piggyback`, `stop_hook`,
  `prompt_hook`, `wait` or `inject`, and when `codex queue` exits 0.
- Set explicitly by the `working` tool.
- Renewed by every feedback take of tier `piggyback`, `stop_hook` or
  `prompt_hook`, by `POST .../working/renew` (the `PostToolUse` hooks and
  Pi's `tool_call`), by the `working` tool, and by the session's agent replies
  and resolves. The shim's session heartbeat, `wait` and `inject` polls and
  `codex queue` never renew.
- Narrowed by the session's agent reply or resolve on a thread the record
  names: that thread leaves the record. A record left with no threads, after
  having named some, is cleared.
- Cleared by: the session's agent reply or resolve on the last thread a
  record names; a publish of the artifact by the session; `done: true`; the
  turn ending (the Stop hook allowing the stop, Pi's `agent_end`); the
  session ending; the artifact's deletion. A viewer resolve or a delete takes
  the thread out of every record, with the same last-thread rule.

What cannot be automatic, per harness, is listed in `docs/contract.md`
("Working").

### Version changelog

Each version may carry a `note` and a set of threads it addressed. A
publish by a session links the threads of that session's working record on
the artifact (then clears the record); `addresses` names more; an agent
resolve links the thread to the current version when it has no link yet.
Linking never resolves a thread; resolving stays a separate act (a viewer
from any card, or an agent with `comments_resolve`). `addresses` may name
resolved threads; a thread that is not the artifact's is 400
`unknown_thread`.

### Batch send

The viewer can send several threads at once (`POST .../threads:send`). One
transaction checks every thread and writes every feedback row, marked with
the batch; one fan-out follows, so every tier hands the batch over together.
The payload leads the batch's comments with `[clax] N comments on "<title>",
sent together by <name>.` and ` Note: "<note>"` when there is one. Each
thread keeps its own rows, sent state, working marker and changelog link.
A batch holds 1 to 20 threads, its note at most 280 characters, and goes to
one target as a single send does (`to`, or the owner and watchers without
it).

### Participants and attention

A viewer is in a thread when they wrote a comment in it (`author_public_id`),
a comment in it @mentions them (`@` and their whole display name, any case,
not preceded by a letter or digit and followed by the end, whitespace or
punctuation, so a two-word name needs both words: `@Mia Kovač`; `@agent`
names no viewer), or they resolved it. For each
artifact the daemon computes, per viewer: `addressed` (open threads they are
in linked to a version after they last looked at the thread), `new_replies`
(threads they are in with someone else's comment newer than their last
look), `open_in`, and `seen` (`viewer_seen`). Looking at a thread is its card
being at least half visible for a second, or selecting it; it writes
`viewer_threads`. Viewing the latest version at the artifact's latest URL
(`/a/<aid>`) writes `viewer_seen`, never past the latest version; a `/v/<n>`
view writes nothing. Resolving
is never needed to clear anything. `seen` is public: `participants.people`
carries each person's, for the people panel. Looked-at marks and attention
stay private to their viewer.

The agents on an artifact are its owner session, the sessions watching it,
and the sessions that published its versions, at most 10, each identified by
`agent_handle` and named in the shell by its harness. An agent is `live` when
its session is live and is the owner or a watcher, so a send can reach it.
They are listed live first, then most recently active first: the newest of
its versions of the artifact, its comments on the artifact's threads, and
its watch, else its registration.

Send, single or batch, takes an optional `to` (an agent handle); a `to` that
names no live agent of the artifact is 400 `unknown_agent`, and nothing is
written. The shell picks `to` for every send: the agent this viewer last
sent to on the artifact (remembered per browser) if it is live, else the
first live agent in the list above (the most recently active live owner or
watcher). With no live agent the shell sends without `to`, and the comments
wait untargeted for the next session that publishes or watches (Data flow,
step 2).

### Presence

A viewer with the artifact open reports `here` (tab visible) or `away`
(hidden, or 5 minutes without input) every 30 s, and optionally `where` (the
anchor label of the thread they have selected or are writing on, at most 80
characters; the people panel's "Share where I'm looking" switch, on by
default and stored per browser, stops it), and `away` as they leave the
page. The daemon keeps reports in memory; one lapses 90 s after the last,
shows as "last here" for 10 minutes, then goes. Changes go out as `presence`.
An artifact lists at most 64 people: a newcomer takes the place of the gone
person whose last report is oldest, and with none gone its report is 429
`limit_reached` (the shell tries again with its next report).

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
receive it. `clax hook --agent <x> session-start` therefore registers
`{harness, harness_session_id, cwd, parent_pid, ancestor_pids}` and the
daemon joins the two records on `(harness, parent_pid)`, trying the hook's
ancestors nearest first when its parent is a wrapper shell (Codex runs hooks
under `bash`), since both the shim and the hook descend from the same harness
process. Where a harness exposes
the session ID in the shim's environment, the shim sends it and the join is
by ID instead. Claude Code additionally sets `CLAUDE_CODE_SESSION_ID` for
hook processes, as toolpath's plugin relies on.

Grok Build passes `GROK_SESSION_ID` to the stdio MCP servers it spawns
for a session, and to hooks (with the session ID in their input as
`sessionId` and `session_id`). The shim under `--agent grok` registers
with that ID and its own working directory (Grok's), so the row is keyed
by the Grok session ID from the start and hooks join by ID. In Grok's
default mode the shim's parent is `grok`; in leader mode it is the
leader process. Subagents share the parent session's MCP servers, so
their clax calls count as the parent session.

The registered `cwd` is `CLAUDE_PROJECT_DIR` (else the shim's own working
directory) under Claude Code. Pi has no shim: its extension registers
`{harness: "pi", harness_session_id, cwd, pid, parent_pid}` itself (§13).
Codex starts the shim in the plugin's directory, so under Codex the shim
registers its parent process's working directory (the Codex session's), read
from `/proc/<ppid>/cwd` on Linux and `lsof` on macOS, or an empty `cwd` when
that fails; the `session-start` hook then fills an empty one.

Heartbeats: the shim `PATCH`es `last_seen_at` every 60 s; a session with no
heartbeat for 5 minutes and a dead PID is marked ended. The session
heartbeat keeps the session row alive only; it never renews working records
(§10, "Working"). Registration assigns `agent_handle`.

## 12. MCP tool surface

Exposed by the shim (stdio) and the daemon (HTTP). Names are shared across
harnesses; each harness prefixes them: Claude Code shows them as
`mcp__plugin_clax_clax__<name>` when installed as a plugin and as
`mcp__clax__<name>` from a plain `.mcp.json` entry named `clax`, Codex
as `mcp__clax__<name>`, Grok Build as `clax_grok__<name>` (reached
through its `search_tool` and `use_tool` meta-tools), and the Pi
extension registers them as `clax_<name>`.

Artifacts: `publish` (file_path or html, files map, title, description,
icon, capabilities, url to update, if_version, label; note (the version's
change note), addresses (thread IDs this version addresses); creating an artifact
needs a title, which the tool takes from the page's first `<title>` when
`title` is omitted and refuses with `invalid_args` when there is neither),
`read` (url or id, optional path), `list` (limit, scope mine|all, files
scope), `delete`, `open` (opens the browser on the machine running the tool
and reports `opened` from the opener's exit status within 1.5 s), `pin`,
`unpin`, `asset_upload` (file_path or file_paths).

Comments: `comments_read` (url_or_id, thread_id, cursor, include_resolved),
`comments_reply` (url_or_id, thread_id, text), `comments_resolve`
(url_or_id, thread_id; an agent resolve links the thread to the current
version when it has no link yet), `working` (url_or_id, thread_ids, message,
done), `watch` (url_or_id, on, replies), `wait_for_feedback`
(url_or_id optional, timeout_s default 50, raised to at least 1 and capped
at 600). The artifact argument keeps the phase 2 name `url_or_id`.

Data: `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`,
`db_str_replace`, `db_batch`, with `collection`, `doc_id`, `data`,
`file_path`, `if_version`, `as_level` as on claude.ai.

Server: `status` (daemon URL, version, this session's ID and watches, and
`push`: whether tier 5 reaches this session and why not).
Under Claude Code, `push` also carries `channel` (whether the launch flag
names the channel, and how to launch with it) and, when no channel
forwards notices, `follow_command`, the background command the skill runs.

Every tool result is JSON text plus, when present, a trailing
`---\n[clax] N comments sent to you:\n...` block (`1 comment` when N is
1; tier 1). URLs in
results are always the daemon's browser URL so they can be pasted to a
person.

The CLI exposes the same operations as `clax <cmd> --json` for harnesses
without MCP and for scripts, including `clax read` and `clax asset
upload`, whose `--json` output is the `read` and `asset_upload` tool's result
object. The other commands print their own JSON shape.

## 13. Plugins

### Claude Code (`plugins/claude-code`)

- `.claude-plugin/plugin.json`: name `clax`, keywords, version, and
  `channels: [{"server": "clax"}]`, which lets a session launched with
  `--dangerously-load-development-channels plugin:clax@clax` register the
  `clax` server as a channel (§10, Notices).
- `.mcp.json`: `{"clax": {"command": "${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh", "args": ["exec", "mcp", "--agent", "claude"]}}`.
- `hooks/hooks.json`: `SessionStart` → `hook --agent claude session-start`
  (registers, prints the daemon URL and any pending feedback as context);
  `UserPromptSubmit` → `hook --agent claude prompt` (tier 3); `Stop` →
  `hook --agent claude stop` (tier 2, timeout 10 s; when it allows the stop it
  ends the turn's working records); `PostToolUse` → `scripts/tool-hook.sh claude`
  (renews working records at most once a minute per session: a shell check
  of a stamp file under `~/.clax/run/tool-hook/` skips starting `clax` when
  the last renewal is under 60 s old; always exits 0; timeout 5 s); `SessionEnd` →
  `hook --agent claude session-end`.
- `skills/clax/SKILL.md`: when to publish, the page contract (title,
  tokens, dark mode, phone width, storage in try/catch, CDN guidance),
  capability usage with the `.d.ts` files as references, the
  comment-driven loop, the `wait_for_feedback` loop convention, and the
  tier 5 fallback (start `push.follow_command` in the background after a
  publish when `push.tier` is null).
- `commands/`: `/clax:open [id]`, `/clax:comments [id]`,
  `/clax:watch [id] [off]`, `/clax:wait [id]`, `/clax:serve`
  (start, stop, status, bind for LAN), `/clax:doctor`.
- `scripts/ensure-clax.sh`: a thin wrapper. It runs `$CLAX_BIN`, else the
  first `clax` on `PATH` that reports itself as clax; it never downloads,
  builds, or looks anywhere else. A binary whose version differs from the
  plugin's (`CLAX_VERSION` in the wrapper) runs; MCP and CLI modes warn about
  it, hooks stay silent. MCP mode runs `clax mcp --preflight` (the home, its
  `config.toml` and the port; no daemon, no network), then execs `clax mcp`,
  so the harness is the shim's parent (§11). With no usable binary, or a
  failed preflight, MCP mode answers the MCP client with a minimal server
  whose one tool, `status`, states the reason. A `clax mcp` that exits later
  in the session is not relayed: the client sees the connection close. With
  no usable binary, hooks print one line and exit 0. Every failure and every
  MCP start is one line in `~/.clax/logs/hooks.log`.
- Installation: `clax init` writes the plugins embedded in the binary to
  `~/.clax/marketplace/` and registers them (`claude plugin marketplace
  add`, `claude plugin install clax@clax`); `clax uninit` removes them.
  `just dev claude` loads the checkout's plugin with `--plugin-dir`.
- Root `.claude-plugin/marketplace.json` lists it.

### Codex (`plugins/clax`)

The directory is named after the plugin because a Codex marketplace entry
must point at `./plugins/<plugin-name>`.

- `.codex-plugin/plugin.json`: name `clax`, version, `skills: "./skills/"`,
  `mcpServers: "./.mcp.json"`, and the `interface` block (display name,
  descriptions, developer, category, capabilities, three `defaultPrompt`
  strings). The manifest has no `hooks` key: Codex rejects it there and
  discovers `hooks/hooks.json` by convention.
- `.mcp.json`: `{"mcpServers": {"clax": {"command": "bash", "args":
  ["./scripts/ensure-clax.sh", "exec", "mcp", "--agent", "codex"], "cwd":
  "./", "env_vars": [...]}}}`. Codex expands no plugin-root variable in
  `.mcp.json` but resolves a relative `cwd` against the installed plugin
  root. It starts MCP servers with a minimal environment (which keeps `PATH`),
  so `env_vars` forwards `CLAX_HOME`, `CLAX_NO_OPEN`, `CLAX_BIN` and
  `CLAX_CODEX_BIN` (which a daemon the shim starts inherits, §10).
- `hooks/hooks.json` in Claude Code's format: `SessionStart` →
  `bash "${PLUGIN_ROOT}/scripts/ensure-clax.sh" exec hook --agent codex
  session-start` (joins the session, records `CODEX_HOME`, and prints the
  daemon URL and any pending `prompt_hook` feedback as context; Codex has no
  `UserPromptSubmit` hook wired), `Stop` → the same with `stop` (tier 2, timeout 10 s; the
  hook gives up after 8 s; it also ends the turn's working records when it
  allows the stop), `PostToolUse` → `bash "${PLUGIN_ROOT}/scripts/tool-hook.sh"
  codex` (the same once-a-minute renewal; not yet measured on Codex, see
  `docs/contract.md`), `SessionEnd` → the same with `session-end` (Codex
  caps `SessionEnd` at 3 s, so `session-end` gives up after 2.5 s). Hooks run
  in a shell with `PLUGIN_ROOT` exported.
- `skills/clax/SKILL.md`: same content as the Claude skill, with Codex
  tool naming (`mcp__clax__<tool>`).
- `scripts/ensure-clax.sh`: a copy of the Claude plugin's wrapper.
- Root `.agents/plugins/marketplace.json` lists it. Installed by `clax init`
  (`codex plugin marketplace add ~/.clax/marketplace`, `codex plugin add
  clax@clax`). Codex has no flag that loads a plugin from a directory: it
  loads plugins from its install cache (`$CODEX_HOME/plugins/cache/`).
  `just dev codex`, like every `just dev`, puts the fresh build first on
  `PATH` and runs on the dev home `~/.clax-dev`; Codex then loads the Clax
  plugin from its install cache.

Person-side settings, documented in the plugin README rather than set by the
plugin:

- Hooks run only with `features.hooks = true` and after the person trusts
  them (Codex asks in an interactive session; `codex exec` skips untrusted
  hooks). Without hooks the tools work and the session is registered by the
  shim alone.
- Codex asks before each MCP tool call, and `codex exec` (approval policy
  `never`) refuses such calls. `[plugins."clax@clax".mcp_servers.clax]
  default_tools_approval_mode = "approve"` approves every clax tool,
  including `delete`.

### Grok Build (`plugins/clax-grok`)

The directory is named after the plugin. The plugin is not named `clax`,
because Grok also discovers the Claude Code plugin of that name
(`~/.claude/plugins`, User scope, disabled until the person enables it)
and resolves plugin-name conflicts before enabling.

- `.grok-plugin/plugin.json`: name `clax-grok`, version, description.
  Grok finds the other components by convention.
- `.mcp.json`: `{"mcpServers": {"clax_grok": {"command":
  "${GROK_PLUGIN_ROOT}/scripts/ensure-clax.sh", "args": ["exec", "mcp",
  "--agent", "grok"], "env": {"GROK_PLUGIN_ROOT": "${GROK_PLUGIN_ROOT}"}}}}`.
  The server is named `clax_grok`, not `clax`: Grok keeps the first MCP
  server definition of a name and drops the rest, so a shared name would
  let the Claude Code copy's server shadow this one. Grok passes its whole
  environment, plus `GROK_SESSION_ID`, to stdio servers.
- `hooks/hooks.json` in Claude Code's format, each command
  `"${GROK_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec hook --agent grok
  <event>`: `SessionStart` → `session-start` (joins by `sessionId`, fills
  `cwd`, prints nothing: Grok ignores its output), `Stop` → `stop`
  (tier 2, timeout 10 s; acts only when `reason` is `end_turn` or absent,
  reads `stopHookActive`; allowing the stop ends the turn's working records),
  `SessionEnd` → `session-end` (timeout 2 s;
  `session-end` gives up after 1.2 s, inside Grok's 1.5 s default). No
  `UserPromptSubmit` hook: Grok discards an allowing one's output. No
  `PostToolUse` hook: a Grok session's working records are renewed by its
  feedback takes, its replies and resolves and the `working` tool, and
  otherwise lapse as in §10 "Working".
- `skills/clax/SKILL.md`: the same skill as the other plugins, with tools
  named `clax_grok__<tool>` and called through `use_tool`, plus a "Live
  feedback in Grok" section: after its first publish in a session, unless
  `clax_grok__status` reports `push.available`, the agent starts Grok's
  `monitor` tool with `persistent: true` on `"<binary.path>" feedback
  follow --agent grok --harness-session <session.harness_session_id>`
  (both from `status`), and answers each line by calling
  `clax_grok__comments_read` on the thread it names.
- `scripts/ensure-clax.sh`: a copy of the wrapper.
- Installed by `clax init` (`grok plugin uninstall clax-grok --confirm`,
  then `grok plugin install ~/.clax/marketplace/plugins/clax-grok --trust`);
  `clax uninit` uninstalls it. Clax never runs a `grok` command naming
  `clax`, which in Grok is the Claude Code plugin's install. Root
  `.grok-plugin/marketplace.json` lists it, so a Grok marketplace added
  from `~/.clax/marketplace` offers clax-grok, not the Claude Code plugin.
- `just dev grok`, like `just dev codex`, runs the installed plugin with
  the fresh build first on `PATH`, on `~/.clax-dev`.

**One acting copy.** In a Grok session only `--agent grok` acts. The
wrapper and the binary both treat a `--agent claude` run as started by
Grok when, for a hook, `GROK_HOOK_EVENT` is set (or the input carries
Grok's `hookEventName`), or, for the MCP server, `GROK_SESSION_ID` is set
and `CLAUDE_PID` is not the server's parent process (Claude Code sets
`CLAUDE_PID` to its own PID, so a Claude Code session nested in a Grok
shell still acts). Such a hook reads its input, prints nothing and exits
0. Such an MCP server completes the handshake and offers one tool,
`status`, which says that Clax runs from clax-grok and how to install it
or disable the idle copy (`isError: false`). Each stand-down appends a
`standdown mode=<hook|mcp> agent=claude host=grok` line to
`~/.clax/logs/hooks.log`.

Person-side settings, documented in the plugin README rather than set by
Clax:

- Grok asks before each MCP tool call in its default `ask` mode;
  `[permission] allow = ["MCPTool(clax_grok__*)"]` in
  `~/.grok/config.toml` approves every Clax tool, including `delete`.
  Headless `grok -p` needs `--always-approve` or that rule.
- Grok's sandbox, when turned on, covers the MCP server, the hooks and any
  daemon they start. Start the daemon outside Grok (`clax serve`) or use a
  custom profile with `read_write = ["~/.clax"]`.

### Pi (`plugins/pi`)

- npm package `@empathic/clax-pi` with `pi.extensions: ["src/clax.ts"]`
  and `pi.skills: ["skills"]` (Pi reads only the resources a `pi` manifest
  lists once one exists). Installed by `clax init`, which runs
  `pi install ~/.clax/marketplace/plugins/pi` on the copy embedded in the
  binary. It needs the `clax` CLI on `PATH` or `CLAX_BIN`.
- The extension runs `$CLAX_BIN`, else `clax` on `PATH`, and never
  downloads. `just dev pi` loads the checkout's extension and skill with
  `-e` and `--skill` (and `-ne`, so an installed copy does not load twice).
- `src/clax.ts` registers `clax_<tool>` for the twenty-three tools through
  `registerTool`, with TypeBox schemas mirroring `tools.rs` and results
  identical to the MCP tools. The package carries the Clax version, so
  `status` compares the daemon's version with the Clax version and
  reports `daemon_version` only on real skew. The tools call the daemon's
  REST API over
  `node:http`, finding the daemon through `daemon.json` and starting it with
  `clax serve` when none is running. A tool error is thrown, so Pi marks the
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
- Working: `tool_call` renews the session's working records (at most once
  every 15 s), and `agent_end` ends them.
- The `/clax open [id] | list | status` command.
- `skills/clax/SKILL.md`: the same skill as the other plugins, with
  `clax_<tool>` names, relative file paths resolved against the Pi session's
  working directory, and the `/clax` command.

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

- Default bind `127.0.0.1`. `clax serve --bind 0.0.0.0` opts into LAN
  (`config.toml` sets no bind address). The gallery header shows the LAN URL when
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
  `/api`; the shell, content, and `/healthz` are not checked, except that
  `/a/…` embeds its bootstrap and frame only for a `Host` this rule admits
  (§8 Time to usable). LAN viewers can
  view, comment, send to agent, and resolve; once they have set a display
  name they hold `interact` and may also write `db` docs at that level,
  reopen or delete threads; unnamed viewers hold `view`. No LAN viewer can
  publish, delete artifacts, upload assets, or write `admin`-level docs.
- The viewer routes (creating a thread, commenting, sending to the agent,
  resolving, reopening, deleting, `GET`/`PUT /api/viewers/me`) refuse a request whose `Origin` is
  not the daemon's own (`http://` plus the request's `Host`; artifact
  origins, `null`, and other origins are refused) with 403
  `forbidden_origin`, so a published page cannot call them itself; it
  writes only through the shell's `comments` capability, after the viewer's
  consent. Requests without an `Origin` header are allowed, unless their
  `Sec-Fetch-Site` is `same-site` or `cross-site` (a page's `<img>` or
  other no-cors GET, which browsers send without `Origin`): those are
  refused the same way, so a page cannot mint viewers through
  `GET /api/viewers/me` either.
  The daemon is HTTP only. `GET /api/push` needs no token but names the
  daemon's `codex` path (which usually contains the user name) only to
  requests with the token.
- Content isolation per D5. In LAN mode content runs with an opaque origin.
  Content on the main origin (`/c/...`) carries `Content-Security-Policy:
  sandbox allow-scripts allow-forms allow-modals allow-popups
  allow-downloads` and `/_blob/...` responses carry
  `Content-Security-Policy: sandbox` (see section 8 for the PDF note), so a
  top-level navigation cannot reach the API same-origin.
- Framing. The shell's pages (`/`, `/a/...`) carry
  `Content-Security-Policy: frame-ancestors 'none'` and
  `X-Frame-Options: DENY`: no other page may frame the shell, lay its own
  content over the consent dialog, or post to it. Content on an artifact
  origin carries `Content-Security-Policy: frame-ancestors 'self'
  http://localhost:<port> http://127.0.0.1:<port>`: the shell (the only hosts
  from which it uses subdomain frames) and the artifact's own pages. Content
  on the main origin names no `frame-ancestors`: a sandboxed page's origin
  is opaque, which no source matches, so a list would refuse an artifact page
  framed by another of its pages; framed elsewhere, such a page reaches
  nothing of the viewer's, since its bridge talks only to a parent at the
  shell's origin. Each is a policy of its own, so a page's `<meta>` policy
  applies in full beside it. So one artifact's page cannot frame another
  artifact's content in subdomain mode (the other origin is not in its
  list), and can in sandbox mode (`/c/...` names no ancestors).
- The frame gate in sandbox mode (§9). The shell posts to the frame with
  target `*`, so `clax:bye`, the gate closing for a document that loaded
  without greeting, and a hello that must name the shown artifact and
  version are what keep live data (room messages, sample streams) from a
  document the frame navigated to; there is no per-document welcome nonce.
  What remains is the time from the next document's commit to the shell's
  handling of the outgoing page's bye, in which a push already in flight
  can reach the new document. Every such push carries data the outgoing
  page was entitled to, and that page chose the navigation (the shell
  closes the gate itself before any navigation it starts), so it could have
  handed the same data over itself; a nonce would not narrow the window,
  since a `*` post reaches whatever document holds the frame. Wherever the
  gate closes, the capability host leaves: the room socket closes and
  `sample` aborts its calls.
- Comment bodies, doc contents, and room messages are untrusted data. Tool
  results render comment bodies only as JSON-escaped strings inside the
  labelled feedback block; `db_*` results return documents as JSON data
  beside an untrusted-text `note` (data cannot be wrapped without changing
  its shape); the skills say so.
- `sample()` spends the configured key. Only the owner's browser (the
  token and a viewer cookie) spends it, and every call waits on consent
  given in that view; the shell shows a running count of calls. The key is
  read from its environment variable once, when the daemon starts, is sent
  only to `base_url` in the `x-api-key` header, and never appears in a
  response, an SSE frame, a log line, an error message or a `Debug` string.
- No telemetry, no outbound calls except `sample()`. The plugins never
  download anything. `install.sh`, which a person runs by hand, downloads
  a release and checks it against the release's `SHA256SUMS`, which comes
  from the same place, so the check protects integrity, not authenticity.
- Working views and events carry the record's `key`, `harness`, `message`,
  thread IDs and times, and are readable without the token, like threads:
  LAN viewers see which harness is working and its message. They never
  carry a session ID, working directory or PID. Messages and version notes
  are agent text, rendered by the shell as text only.
- Participants carry viewers' public IDs and display names (already public
  through comments), each person's last viewed version (`seen`, public by
  design so the people panel can show it), and agents' handles and
  harnesses. Attention and
  looked-at marks are served only to the viewer whose cookie the request
  carries. Presence carries public IDs, display names, here or away, and the
  optional location the person chose to share. An artifact lists at most 64
  people (§10 Presence), and a room socket reads at most 64 KiB of one
  message, so neither grows without bound from callers without the token.
- Consequential actions in the shell take a pointer's click once the
  viewer's keys may have been the page's (§8, "Consequential actions and
  the keyboard trail").
- The shell HTML for `/a/…` embeds only what `GET /api/artifacts/<id>`,
  `GET /api/artifacts/<id>/threads` (without the token) and
  `GET /api/viewers/me` (for the cookie's own viewer, never creating one)
  would answer the same browser, is never served on an artifact origin, and
  never holds the daemon token.

## 15. Error handling

- Daemon unreachable after auto-start: shim tools return a structured
  error naming `~/.clax/logs/daemon.log`; hooks exit 0 with no output so
  the harness is never blocked by clax being down.
- Publish conflict (`if_version` stale): the tool returns the current
  version's content summary so the agent can merge, as claude.ai does.
- Anchor not found: thread marked detached, never dropped.
- Clip failure: thread saved without a clip; the payload says so.
- Hook timeouts: every hook exits 0, silently, when its budget runs out:
  SessionStart 4 s, SessionEnd 2.5 s (Codex kills it at 3 s), Stop 8 s
  (inside the plugins' 10 s hook timeout), prompt submit 4 s, tool 2 s (1 s per
  daemon request). Each daemon call inside the others uses a 3 s timeout (2 s
  in SessionEnd).
- `wait_for_feedback` past the harness's limit: returns early with a
  "call again" result rather than erroring.
- `codex queue` failure: handled per §10; never retried in a loop, never
  blocks the send request that triggered it (dispatch is asynchronous).
- Storage debris: `clax doctor` reports it and `clax doctor --fix`
  removes stray staging directories and temp files, version directories above
  the current version, zero-version artifact rows, and asset rows and corrupt
  rows belonging to soft-deleted artifacts. Rows of live artifacts are never
  deleted, and `--fix` refuses to run while a daemon is live.
- Request timeout (408 `timeout`): a 408 on a write route means the outcome
  is unknown; read the artifact before retrying.
- Storage corruption: `clax doctor` runs `PRAGMA integrity_check`,
  verifies files against `versions.files_json`, and reports.

## 16. Testing

- **clax-core**: unit tests for IDs, anchors (serialisation only; DOM
  resolution is tested in the browser), storage round-trips, rules
  evaluation for `db`, version copy-forward semantics.
- **clax-server**: integration tests with a temp `CLAX_HOME`, real
  SQLite, `axum` test client: publish, versions, files, assets, threads,
  send-to-agent → feedback rows, long-poll delivery, `db` rules by level,
  token enforcement per route, working records (set, mark, renew, clear,
  expiry by an injected clock), the changelog links and seen marks,
  attention and looked-at marks, agent handles, the send target, and
  presence (expiry by an injected clock).
- **clax-mcp**: spawn the shim against a test daemon, drive it with an
  MCP client over stdio, assert tool schemas and tier 1 piggyback. The
  `db_*` tools are tested in-process against a test daemon
  (`crates/clax-mcp/tests/db.rs`); the shim test only checks that their
  eight names are listed.
- **clax-hooks**: fixture-driven tests with captured stdin JSON for each
  harness event and golden stdout, including `stop_hook_active`, the
  Codex `stop` shape, Grok's camelCase envelope, the PostToolUse `tool`
  hook's renewal and the working end when a Stop is allowed, in Claude Code,
  Codex and Grok sessions, and the Claude Code copy standing down in a Grok
  session.
- **Browser**: Playwright against a real daemon: gallery, shell, the version
  moment (the version button's dot and the Reload button), comment mode on element and range, clip produced, send to agent,
  agent reply visible via SSE, `window.claude.use` for each capability in
  both origin modes (`*.localhost` and opaque sandbox), db `onSnapshot`,
  self-publish reload, Echo (system type, theme switch, keys, the mark), the
  working summary, roster, card chips, thread marker, pins and capability,
  the Addressed group, version menu and history line, needs your eyes, batch
  send with the agent picker, and presence; every UI change is checked in
  screenshots, light and dark, desktop and phone. `web/e2e/contract.spec.ts` runs the sample pages in
  `web/e2e/pages/`, written for claude.ai's contract 0.2.61, unchanged in
  both frame modes; `scripts/smoke-capabilities.sh` checks the capabilities
  end to end against a scratch daemon (not a quality gate).
- **Plugins**: shell tests for `ensure-clax.sh` (the `PATH` lookup, the
  fallback MCP server, hooks), and tests of `clax init`/`uninit` and
  `just dev` against fake `claude`, `codex`, `grok` and `pi` commands and scratch
  harness configuration directories; a Claude Code smoke test
  that loads the plugin from the repo path and runs a scripted session;
  structure checks and Codex's plugin validator for the Codex plugin, and a
  manual Codex smoke test that installs it into a scratch `CODEX_HOME`; a fake Grok (`crates/clax-cli/tests/grok_dedupe.rs`) that loads both Clax plugins with Grok's first-definition-wins server merge and checks that one copy acts in every combination; a manual Grok smoke test (`scripts/smoke-grok.sh`); the Pi extension through a fake
  `ExtensionAPI` object against a real daemon, plus a manual `pi -p` smoke
  test (`scripts/smoke-pi.sh`).
- Claude Code channel: a fake `claude` (a script whose command line
  carries the launch flags, and which runs `clax mcp --agent claude` as its
  child) drives the shim over raw JSON-RPC. The tests check the capability,
  the protocol cap, one event per comment, silence without the flag, one
  announcement across the channel and `clax feedback follow`, and the pause
  inside `wait_for_feedback`. `scripts/smoke-claude-push.sh` is the owner's
  live check.
- **Release**: `scripts/test-release.sh` checks the version, bump and
  packaging scripts; `scripts/test-install.sh` runs `install.sh` against a
  local fake release server; `.github/workflows/release.yml` builds,
  smoke-tests and packages every target on pull requests that touch it and
  on manual runs, and publishes only on a `v*` tag.
- `scripts/quality_gates.sh` runs fmt, clippy `-D warnings`, cargo test,
  web lint (oxlint) and typecheck, Playwright, and the plugin tests; CI runs
  the same script.
- Shell unit tests use Vitest with jsdom and `@testing-library/svelte`.
  `svelte-check --fail-on-warnings` runs with the typecheck. Two gates hold
  time to usable: `web/perf` (Playwright timing, budgets per platform in
  `web/perf/budget.json`, enforced where the platform has budgets and
  report-only with a warning elsewhere; §8 Time to usable) and
  `web/scripts/bundle-size.mjs` (gzip sizes of
  each entry's critical JavaScript and of the eager bridge, budgets in
  `web/perf/bundle-budget.json`).
- Two gates hold the daemon under load, each on a scratch release daemon
  with budgets scaled by the run's own idle baseline:
  `scripts/perf-daemon.sh` (cheap requests stay fast beside heavy ones,
  `scripts/perf-daemon-budget.json`) and `scripts/perf-clients.sh` (1,000
  `/api/stream` clients: write-to-client latency, cheap requests under that
  load, RSS per client, idle CPU, and a client that never reads getting
  `resync` without the daemon's memory growing,
  `scripts/perf-clients-budget.json`).

## 17. Phases

Each phase is shippable on its own and gets its own implementation plan.

**Phase 1: daemon, publish, versions, gallery, viewer.** Stands alone:
no comments, no sessions, no capabilities, no room, no sample. In scope:
`clax serve` with discovery, auto-start, `daemon.json`, and `stop`;
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
the daemon, `ensure-clax.sh`, Claude Code plugin with skill and
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

Shipped: the room socket with levels, declared `interact` topics, named
rooms, a shared budget and bounds, and replacement by label; `room` and
`sample` as lazy bridge parts with shell handlers loaded on first use; the
`[sample]` table with an Anthropic provider and a stub, an invalid table
turning sample off with a `warn` line in `clax doctor`; the sample routes
behind the owner gate, streamed over SSE with page tool rounds and
cancellation; per-browser answer caching, a per-browser queue and the
optional daily cap; consent once per view in the owner's browser and the
running count of today's calls; the socket and calls ending when the
frame's document leaves; and `just demo-room-sample`, which runs both demo
pages in a scratch daemon with the stub provider unless `ANTHROPIC_API_KEY`
is set.

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
- **`sample()` cost control.** Resolved: phase 5 builds the optional
  per-artifact `daily_call_cap` in `config.toml` (section 9). It is off by
  default: only the owner's browser can spend, and every view asks first.
- **Grok Build.** Resolved 2026-10-01: Clax does not write Grok's
  tool-approval rule (the plugin README documents it and `clax doctor
  --agent grok` prints it); sandboxed Grok is documented, not handled
  (§13); Grok Build 1.0.45 is the minimum, which `clax doctor --agent grok`
  warns about and nothing enforces at runtime; there is no `PostToolUse`
  hand-over. Open: the live checks in `scripts/smoke-grok.sh`, which the
  owner runs; until they pass, Grok's tiers are stated from its source.
- **Channel registration is invisible to the server.** Claude Code neither
  acknowledges a channel event nor says whether it registered the server.
  The shim infers the channel from the launch flag. If Claude Code ever
  exposes registration (a client capability in `initialize`, or an
  acknowledgement), `status` should report it in `registered`, and the
  shim should poll only when registration is confirmed.
- **Channels are a research preview.** The flag syntax and the
  notification contract may change. Clax's channel code is in
  `crates/clax-mcp/src/channel.rs`, so a change lands in one place.
