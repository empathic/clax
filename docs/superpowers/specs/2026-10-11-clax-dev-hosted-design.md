# clax.dev: hosted Clax on Cloudflare Workers and Artifacts

Date: 2026-10-11
Status: prototype design; binding for the `clax-dev` branch
Depends on: `2026-09-28-clax-design.md` (the artifact, version, thread and
feedback model), `2026-10-08-multi-agent-multi-person-design.md` on the
`multi-person` branch (people, rights presets, the visibility filter,
approvals, agent names, thread ownership).
Target: the Cloudflare "next Git platform" competition
(https://blog.cloudflare.com/next-git-platform-on-cloudflare/), due
2026-10-14: Workers and Artifacts, several agents working on changes at
once, a 5–10 minute video, permissively licensed source (this repository is
Apache-2.0), instructions to run it.

## 1. Purpose

Clax today is local: one person's daemon, their agents, their browser. This
prototype puts the same model on the web so several people and their agents
ideate and collaborate on one artifact:

- An agent (any harness that speaks remote MCP) publishes an HTML artifact
  and gets a link anyone it is shared with can open.
- People comment on the page (whole page or a text selection), and send a
  thread to a named agent. The agent reads it, publishes a new version,
  replies and resolves.
- Several agents, of one person or of several, work on artifacts at the same
  time: publishes are serialized per artifact, a stale publish is refused
  with the current version so the agent merges and retries, and every agent
  sees who else is working.
- Comments written by someone other than an agent's own person never reach
  that agent until its person approves the author (per thread, per artifact,
  or everywhere): the prompt-injection guard of the multi-person design.
- Anyone can **fork** an artifact into a new artifact of their own that
  records where it came from. The original lists its forks.
- Every artifact is a git repository in Cloudflare Artifacts, one commit per
  version. People and agents with the right can clone it with git or jj; the
  artifact's owner may also push to it, and a push becomes a version.

### Non-goals

- Changing the local daemon. Syncing a local Clax with clax.dev is later
  work (§13).
- Runtime capabilities (`db`, `room`, `downloads`, …), live pages, the
  Chrome extension, questions and the inbox beyond approvals.
- OAuth or passwords. People join by invite link (§6).
- Merging a fork back. A fork is a new artifact; nobody's view is a fork.

## 2. Decisions

| # | Decision | Reason |
|---|---|---|
| H1 | **TypeScript Worker** in `hosted/`, Durable Objects with SQLite storage, Workers static assets for the UI. | The Artifacts binding is a Workers API; `clax-core` needs rusqlite and a filesystem. |
| H2 | **One Artifacts repo per artifact**, named `a-<artifact ID>`; **one commit per version** on `main`, tagged `v<n>`. | The version model is already immutable snapshots of a file set. |
| H3 | **The Worker writes commits over the git smart-HTTP protocol** (receive-pack) with a write token it mints for itself, building only the new blob, tree and commit objects (unchanged subtrees are referenced by hash from `readTree`). Built and tested against fakes of the binding and against `git http-backend`; never yet run against Artifacts. | The binding reads repos but has no write call. |
| H4 | **`RepoStore` interface** with two implementations: `ArtifactsRepoStore` (the binding plus H3; tested against fakes and `git http-backend`, never against Artifacts) and `MemoryRepoStore` (real git object hashing, every repository's objects and refs in one Durable Object, `RepoDO`) for tests and `wrangler dev` without an account. Chosen by the `REPO_STORE` var (`artifacts` / `memory`). | No Cloudflare account yet; tests must not need the network. |
| H5 | **One `ArtifactDO` per artifact** holds its metadata, version index, threads, comments, feedback queue, working records, forks and the WebSocket clients; one `DirectoryDO` (a single instance, name `directory`) holds people, invites, browser sessions, agent tokens, agents, approvals and the gallery index; with the memory store, one `RepoDO` (a single instance, name `repos`) holds every repository. | Per-artifact serialization (H7) and cheap fan-out; the directory is small. |
| H6 | **Identity:** a person is a row in the directory. The first person is made by `/setup?secret=<SETUP_SECRET>`; others by one-use invite links (24 h). A browser holds an `HttpOnly`, `Secure`, `SameSite=Lax` session cookie that lives 30 days (`Secure` is dropped for `http://localhost` and `http://127.0.0.1`, so local development signs in over plain HTTP). An agent holds a bearer **agent token** a person mints in the browser ("Connect an agent"); one token per person per machine, many agent sessions under it. | GNAP's grant-per-machine (multi-person M3) without the LAN key binding: TLS carries the token, so the multi-person spec's `clax-ed25519` signing (M5) is not needed here. |
| H7 | **Optimistic concurrency.** `publish` takes `if_version`; the ArtifactDO serializes writes; a stale `if_version` is 409 `version_conflict` carrying the current version number, its files list and its publisher, and the agent re-reads, merges and retries. | Today's Clax contract; it is the "several agents at once" rule. |
| H8 | **Rights** are per person per artifact, one of the roles `viewer`, `commenter`, `editor`, `owner` (§7). An artifact's creator is its owner. Sharing grants `viewer`, `commenter` or `editor` to a person, or to "anyone with the link"; `owner` is the creator's alone. An agent has its person's rights. | The multi-person presets, narrowed to what the prototype shows. |
| H9 | **Visibility** (§8): every author field an agent channel returns passes one filter. Withheld content is replaced by a placeholder; the agent may ask its person for approval. | Multi-person O3, M10, M12, M13. |
| H10 | **Git tokens follow rights and visibility** (§9): a token needs `viewer` (a signed-in person or their agent) and, for an agent, read or write, every author in the repo's history visible to it; a write token is only ever minted for the artifact's owner (person or their agent). | Repo-scoped tokens cannot express rights or filter files; owner-only writes make a push's author the owner, authentically. |
| H11 | **Forking** creates a new artifact owned by the forker from a chosen version, with `repo.fork()` (history kept), records `forked_from {artifact_id, version}`, and copies no threads. The fork's v1 keeps the original's title, title author and file authors. | A fork is someone's own copy to take elsewhere. |
| H12 | **Content isolation** (`CONTENT_ORIGIN` requires `PUBLIC_ORIGIN`): artifact files are served from `CONTENT_ORIGIN` (a separate hostname in production) or, when unset, into an `<iframe sandbox="allow-scripts allow-forms allow-popups allow-modals">` without `allow-same-origin`, with a CSP of `default-src 'unsafe-inline' data: blob: <content prefix> https:; connect-src 'none'; frame-ancestors <PUBLIC_ORIGIN, else 'self'>; sandbox allow-scripts allow-forms allow-popups allow-modals` (so content opened at top level also runs in an opaque origin, and only the app may frame it): a page may load scripts, styles, fonts and images from any HTTPS origin (pages use CDN libraries) but may not connect anywhere. Its requests leave an opaque origin, so the app's `SameSite=Lax` cookie never rides along. Served HTML has the selection script (`src/content-selection.ts`) injected before its last `</body>`, which posts text selections to the app for anchored comments. The app's own HTML pages send `frame-ancestors 'none'`. | A published page must never act as the viewer. |
| H13 | **Remote MCP** at `/mcp` (Streamable HTTP), authenticated by the agent token. | Any harness connects with one line. |

## 3. Architecture

```
browser ──HTTPS──▶ Worker (hosted/src/index.ts)
  │                  ├─ /api/*        JSON API (§10)
  │                  ├─ /mcp          remote MCP (§11)
  │                  ├─ /c/*          artifact content (H12)
  │                  ├─ /git/*        the memory store's git remote (§9)
  │                  ├─ /a/:id, /     the UI (static assets, Svelte)
  │                  └─ queue consumer: Artifacts event subscription (§9)
  ├─ WebSocket ─────▶ ArtifactDO(id)  threads, versions, live events
  │                  DirectoryDO      people, tokens, approvals, gallery
  │                  RepoStore ──────▶ Cloudflare Artifacts (git), or RepoDO (memory)
agent ──MCP/HTTPS──▶ /mcp ──▶ ArtifactDO / DirectoryDO
git, jj ──smart HTTP + repo token──▶ Artifacts remote, or /git/* (memory)
```

The Worker routes; it holds no state. Every write to an artifact goes
through its ArtifactDO, which calls `RepoStore` and the DirectoryDO.

## 4. Repository layout

```
hosted/
  package.json, tsconfig.json, wrangler.jsonc, vitest.config.ts
  src/
    index.ts            Worker entry: routing, auth middleware
    model.ts            shared types (§5) and error codes (§12)
    repo/               RepoStore, MemoryRepoStore and its RepoDO, ArtifactsRepoStore, git objects, pack writer, receive-pack client
    auth/               sessions, invites, agent tokens, rights
    visibility/         the filter and placeholders
    do/artifact.ts      ArtifactDO
    do/directory.ts     DirectoryDO
    api/                HTTP handlers
    mcp/                MCP server and tools
    content.ts          content serving and its cache (H12, §9)
    content-selection.ts the selection script injected into served HTML (H12)
    git/                git tokens and clone commands, the push event consumer, the memory store's smart HTTP remote (serve.ts, upload-pack.ts)
  ui/                   Svelte 5 + Vite app, built into hosted/dist/ui
  test/                 vitest (workers pool) and node tests
  e2e/                  Playwright against `wrangler dev`
  scripts/              demo: seed, concurrent agents
```

## 5. Data model

IDs are 12-character lowercase base32 (`[0-9a-hjkmnp-tv-z]`, Crockford, no
`ilou`) from 60 random bits. Times are RFC 3339 UTC strings.

**Person** `{id, name, created_at, guest?: true}`; `name` is NFKC, letters, marks,
digits, spaces and `.'-`, 1–32 characters. Names are compared folded (NFKC,
lowercased, NFKC again) and are unique among members and, separately, among
guests. Names start with a
letter or digit, use one writing system (Japanese may mix Han and kana,
Korean Hangul and Han), contain no default-ignorable code points, overlay
marks or more than 2 marks in a row, and are never `Clax`. A guest person (a named anonymous link viewer) may comment but owns nothing, holds no shares, approves nothing and mints no agent tokens; every name Clax shows an agent or an
approver labels a guest `Name (guest)`, and a guest may not take a member's
name (a member may take a name a guest holds). **Agent** `{id, person_id, token_id,
name, harness, last_seen_at, ended_at}`: one MCP session; `name` matches
`[a-z0-9][a-z0-9._-]{0,31}`, never `clax` or `clax-*`, unique among live agents (a clash gets `-2`,
`-3`…), set by the `hello` tool or derived from the MCP `clientInfo.name`; `harness` matches the same pattern. An agent unseen for 10 minutes is
idle: it leaves agent lists and Send menus and frees its name; `initialize`
with the same token and `clientInfo.name` resumes the most recent idle
agent (same ID and name) rather than registering a new one. A resume bumps
the agent's session generation, which the MCP session ID carries, so the
session it resumed from answers 404 and that client registers anew: two live
sessions never share an agent. An agent with a live working record is not
idle.
**AgentToken** `{id, person_id, label, hash, created_at, last_used_at,
revoked_at}`; the plaintext is `cxa_` plus 43 base64url characters, shown
once.

**Artifact** `{id, title, owner_id, created_at, updated_at, head_version,
forked_from: {artifact_id, version} | null, link_access: "none" | "viewer" |
"commenter" | "editor", repo: string, title_author_id}`. The reserved author
ID `clax` (`CLAX_AUTHOR_ID`) names Clax itself, for daemon-written notes and
placeholders. **Share** `{artifact_id, person_id,
preset}`.

**Version** `{n, commit, files: {path: {size, content_type, sha1}},
file_authors: {path: person_id[]}, publisher: {person_id, agent_id | null},
note, created_at, via: "publish" | "git"}`. `n` starts at 1. A file a
publish carries unchanged keeps its authors; a file it writes gets the
publisher's person.

**Thread** `{id, version, anchor: {kind: "page"} | {kind: "text", quote,
prefix, suffix, file}, created_by, resolved, resolved_by, target_agent_id |
null, comments: Comment[]}`. **Comment** `{id, thread_id, author:
{person_id, agent_id | null}, body, created_at}`.

**Feedback** `{id, thread_id, comment_id, agent_id, state: "pending" |
"held" | "delivered", created_at}`: one row per comment per target agent.

**Approval** `{person_id, author_id, scope: "thread" | "artifact" |
"everywhere", artifact_id | null, thread_id | null, created_at}`.
**ApprovalRequest** `{id, person_id (approver), author_id, artifact_id,
thread_id, agent_id, asks, state: "open" | "allowed" | "declined"}`; `asks`
counts the sends and `request_approval` calls that opened or bumped it.

**Working** `{agent_id, artifact_id, message, since}`: an agent says what it
is doing; expires 120 s after its last renewal.

## 6. People and sign-in

- `GET /setup?secret=` makes the first person (name from a form) when the
  directory has none; afterwards 404.
- `POST /api/invites {name, preset?, artifact_id?}` (a signed-in person)
  answers `{url}` (`preset` needs the `artifact_id` of an artifact the
  inviter owns); opening it shows the name, asks the joiner to confirm it
  (the joiner cannot change it), makes the person, grants the optional share and sets the cookie. One
  use, 24 h.
- `POST /api/signin-links` answers a one-use 10-minute link that signs
  another browser in as the caller.
- `POST /api/agent-tokens {label}` answers `{token, mcp_url,
  claude_command, codex_config}` once; `DELETE /api/agent-tokens/:id`
  revokes it and ends its agents.

## 7. Rights

Every person has one role per artifact:

| Role | May |
|---|---|
| `viewer` | read the artifact, its versions and threads; clone; fork into their own artifact |
| `commenter` | viewer, plus start threads, reply, send threads to agents, and resolve or reopen threads they started |
| `editor` | commenter, plus publish versions, rename, and resolve or reopen any thread |
| `owner` | editor, plus share, change link access, delete, and git push |

`owner` is the creator's alone. Sharing grants `viewer`, `commenter` or
`editor` to a person; link access is `none`, `viewer`, `commenter` or
`editor` for anyone with the link. A person's role is the higher of their
share and the link access. An agent's rights are its person's.

A request with no person (anonymous link viewer) has the link's role,
capped at `commenter` as a guest's is, and may not comment until it takes a
guest name (a guest person, §6). A guest is
capped at `commenter`, may not fork, owns nothing and mints no agent
tokens. Cloning needs a signed-in person (a member or a guest) or their
agent: a link alone gets no git token. An agent's git token, read or write,
also passes the visibility check of §9.

A person beyond an artifact's live seats (§16) is read-only there whatever
their role: they read and fork, and may not comment, send, resolve, publish
or rename until they take a live seat.

Stores written before these roles map `reader` to `commenter` and
`collaborator` to `editor`, in shares and link access alike.

## 8. Visibility

"May an agent of person R see a field whose authors are A₁…Aₙ, at artifact
Y, thread T?" Yes when every Aᵢ is R, or Clax itself, or R has an approval
for Aᵢ at scope `everywhere`, or `artifact` Y, or `thread` T.

| Field | Authors |
|---|---|
| comment body | the comment's author person |
| thread anchor (quote, prefix, suffix, file) | the anchored file's authors in the thread's version, and the thread's creator (the quote is whatever the creator sent) |
| version file name and bytes | `file_authors[path]` |
| version note | the publisher's person |
| artifact title | the person who last set it |
| working message | the agent's person |

Withheld fields become `null` with `withheld: true` and `withheld_authors:
[{id, name}]`; a withheld anchor nulls `file` too; withheld version files
are left out of `files` and `file_authors` (counted in `files_withheld`),
and a version withholding anything has `commit: null`. A held line whose
title is withheld reads `on an artifact by <name>`. A declined request is
reopened only by a person's send. A feedback row whose comment body is withheld is `held`, and
the agent gets one placeholder per (author, thread): `[clax] 1 comment from
Mia on "Quarterly Review" (thread 7k2…) is withheld until your person
approves it. Call request_approval with {"thread_id": "7k2…"} to ask.`

A send that names an agent whose person has not approved the thread's
comment authors makes an ApprovalRequest at once. The approver sees it in
the UI's Approvals panel with **Allow this thread**, **Always allow <name>
on this artifact**, **Always allow <name> everywhere**, **Decline**.
Allowing turns the held rows `pending`. Only the agent's own person
approves, only from a browser.

## 9. Git and Artifacts

- **Create:** `RepoStore.create("a-<artifact ID>")`, then the first commit.
- **Publish:** `commitFiles({repo, parent, changes: {path: bytes | null},
  author, message})` → commit hash; then tag `v<n>`. The commit message is
  the note; trailers `Clax-Version: <n>`, `Clax-Publisher: <person ID>`,
  `Clax-Agent: <agent name>` (when an agent published).
- **Read:** content is served with `readFile({ref: <commit>, path})`, HTML
  with the selection script injected (H12), and stored in the Workers Cache
  API (`caches.default`) under a key of the content URL's origin and path
  (artifact, commit and file), a hash of the content format (the injected
  script, the CSP template and the header set) and the framing origin (the
  CSP's `frame-ancestors`), so a repeat request does not reach the
  repository and a deploy that changes how content is served misses the old
  entries (`Cache-Control: public, max-age=31536000, immutable` on
  `/c/<artifact ID>/<commit>/<path>`).
- **Fork:** `repo.fork("a-<new artifact ID>")`, then the new artifact's v1 is the
  forked version's tree, committed on top with `Clax-Forked-From:
  <artifact ID>@v<n>`; its files keep their authors in the original's
  version (H11). The fork's repository deletes the `v*` tags it inherited
  (its history stays reachable from `main`), so its own versions tag
  `v1`, `v2`… without colliding.
- **Tokens:** `POST /api/artifacts/:id/git-token {scope}` (browser) and the
  `git_remote` tool (agent) answer `{remote, token, expires_at,
  clone_command}` with `clone_command` =
  `git -c http.extraHeader="Authorization: Bearer <token>" clone <remote>`;
  a token needs a signed-in person or their agent (§7). People's tokens
  live 1 h. Agents' tokens live 15 minutes, and the artifact revokes them
  before a version whose authors the agent's person cannot see lands, when
  the agent's rights at the artifact drop, and when the agent ends. A
  publish whose planned version a live agent token is blind to revokes that
  token before its commit, and is refused with `repo_unavailable` when the
  revoke fails; recording pushed commits likewise records nothing when such
  a revoke fails, and the next reconcile tries again. A revoke that fails
  is kept on a must-revoke list, retried at every call on the artifact and
  by a 30 s alarm. A pushed commit stays fetchable by a token blind to it
  from the push until the next reconcile revokes the token, a window
  bounded by the token's 15-minute life. A write token is refused to anyone but the
  owner or the owner's agent (403 `owner_only`). An agent's token, read or
  write, is refused (403 `withheld`, with the withheld authors) while any
  author in the repository's history is not visible to it.
- **Pushes:** a push to `main` by the owner becomes versions: on the
  `pushed` event (queue consumer); before a git token is minted; and, as a
  fallback, when the ArtifactDO serves the artifact to a reader, at most
  once every 10 s (`RECONCILE_EVERY_MS`), when `main` has moved past the
  head version. Each new commit on `main`'s first-parent chain becomes a
  version with `via: "git"`, publisher the owner, note the commit message.
  A file whose bytes equal the base version's keeps its authors; any other
  file's authors are the owner plus everyone who wrote the same bytes in a
  recorded version (here or in a fork's original). Commit trailers are
  never read for authorship. A commit matching a pending-publish row (a
  publish whose commit landed but whose version row was not written: same
  parent, same files) is recorded as that publish, crediting its
  publisher, authors and note. More than 100 commits since the head version
  (`MAX_RECONCILE_COMMITS`) collapse into one version of `main`'s head.
  Duplicate events are ignored by commit hash. A push that is not a
  descendant of the last known version is recorded as a version anyway,
  its note saying the history was rewritten. Designed for Cloudflare
  Artifacts and not yet run against it: the memory store refuses pushes,
  so this is tested with commits written straight into the memory store.
- **The local store over git:** with `REPO_STORE=memory` and the
  `GIT_SMART_HTTP` var not `0` (unset or `1` turns it on), the Worker serves each repository's upload-pack at
  `<origin>/git/<repo>.git` (smart HTTP v0, Bearer token minted for that
  repository), so `git clone`, `git fetch` and `jj git clone` work without an
  account; receive-pack answers 403, so pushes need Cloudflare Artifacts.
- **jj:** works through `jj git clone` / `jj git push` with the same
  header in git config; the README shows it.

## 10. HTTP API

JSON; errors are `{error: {code, message, ...}}` (§12). `:id` is an artifact
ID. Browser routes need the session cookie (or link access) and a
same-origin `Origin` on writes.

| Route | Does |
|---|---|
| `GET /api/me` | `{person, agents, tokens}` or 401 |
| `POST /api/signout` | ends the caller's browser session and clears the cookie |
| `POST /api/setup` | `{secret, name}` → person, sets the cookie (§6) |
| `GET /api/invites/:code` | `{name, preset, artifact, expires_at}` for the join page, or 404 |
| `POST /api/invites/:code/accept` | `{}` → person, sets the cookie (the invited name, unchanged) |
| `POST /api/invites` | `{name, preset?, artifact_id?}` → `{url, expires_at}` |
| `POST /api/signin-links` | → `{url, expires_at}` |
| `POST /api/guests` | `{name}` → a guest person for an anonymous link viewer, sets the cookie |
| `POST /api/signin-links/:code/accept` | → person, sets the cookie |
| `POST /api/agent-tokens`, `DELETE /api/agent-tokens/:id` | §6 |
| `POST /api/artifacts/:id/git-token` | `{scope}` → §9 |
| `GET /api/artifacts` | the gallery: artifacts the caller owns or has been shared, most recently updated first (an artifact readable only by its link is not listed) |
| `POST /api/artifacts` | `{title, files}` → artifact (browser publish, used by the demo seed) |
| `GET /api/artifacts/:id` | `{artifact, versions, threads, forks, working, rights, people, agents, shares, forked_from, presence}`: `people`/`agents` name everyone the other fields mention; `shares` only for the owner; `forked_from` carries the original's title (null when unreadable); each thread carries `held: [{agent_id, approver, count}]`; `git: {push}` says whether this instance's store accepts pushes (false for the memory store). The shape is `ArtifactView` in `hosted/src/views.ts`. |
| `PATCH /api/artifacts/:id` | `{title?, link_access?}` (owner), applied together or not at all |
| `DELETE /api/artifacts/:id` | owner |
| `POST /api/artifacts/:id/versions` | `{files, if_version, note}`: `if_version` required; `files` carries changes only (a string, `{base64}`, or `null` to delete); unnamed files are kept; identical bytes keep their authors |
| `POST /api/artifacts/:id/fork` | `{version?}` → new artifact |
| `PUT /api/artifacts/:id/shares/:person` | `{preset}` (owner); `{preset: null}` removes the share |
| `POST /api/artifacts/:id/threads` | `{version, anchor, body}` |
| `POST /api/threads/:tid/comments` | `{body}` (artifact from the thread ID prefix: thread IDs are `<artifact ID>.<ID>`) |
| `POST /api/threads/:tid/send` | `{agent_id}` → `{thread, state: "pending" | "held"}` |
| `POST /api/threads/:tid/resolve`, `/reopen` | |
| `GET /api/approvals` | open requests for the caller, each an `ApprovalView` `{request, author, agent, artifact, thread, comments, held}`: `comments` the author's comments in the thread, `held` how many are held from the request's agent |
| `POST /api/approvals/:rid` | `{decision: "thread" | "artifact" | "everywhere" | "decline"}` |
| `GET /api/people` | members the caller shares an artifact with (and the caller), for the Share dialog: `[{id, name}]`; never guests |
| `GET /api/agents` | live agents visible to the caller (name, person name, working) |
| `GET /api/artifacts/:id/live` | WebSocket: `{t: "version" | "thread" | "comment" | "working" | "presence" | "fork", ...}` |
| `GET /c/:id/:commit/*path` | content |
| `GET /git/:repo.git/info/refs`, `POST /git/:repo.git/git-upload-pack` | the local store's git remote (§9) |

## 11. MCP tools

Server name `clax`. Every result is text for the model plus
`structuredContent`. The agent's identity is its MCP session; `hello` names
it.

| Tool | Input | Does |
|---|---|---|
| `hello` | `{name?, harness?}` | names the agent; returns its name, its person, and how to work with Clax |
| `publish` | `{id?, title?, files: {path: string}, if_version?, note?}` | creates (no `id`) or adds a version; 409 → `version_conflict` with the current version. With `id`, a `title` the caller may not set (no rename right) still publishes the version, and the result carries `title_error` |
| `read` | `{id, version?}` | metadata, versions, files (filtered) |
| `list` | `{}` | the artifacts the agent's person owns or has been shared (titles filtered) |
| `fork` | `{id, version?}` | a new artifact owned by the agent's person |
| `comments_read` | `{id}` | threads, filtered |
| `comments_reply` | `{thread_id, body}` | |
| `comments_resolve` | `{thread_id}` | |
| `wait_for_feedback` | `{timeout_s?: 1–55}` | long-polls this agent's pending feedback and placeholders; delivers and marks `delivered` |
| `request_approval` | `{thread_id}` | asks the agent's person |
| `working` | `{id, message}` / `{id, done: true}` | tells everyone what the agent is doing |
| `git_remote` | `{id, scope: "read" | "write"}` | §9 |

## 12. Errors

`unauthorized` 401, `forbidden` 403, `owner_only` 403, `withheld` 403,
`not_found` 404, `version_conflict` 409, `invalid_argument` 400,
`too_large` 413 (a file over 10 MiB or a publish over 25 MiB: a publish
travels to its ArtifactDO in one RPC, capped at 32 MiB, and Artifacts caps a
blob at 32 MB),
`repo_unavailable` 503.

## 13. Later work, stated in the writeup

Splitting the directory (sessions and tokens by hash, approvals and
galleries per person) so 100,000 people across all artifacts never meet in
one object; syncing a local Clax daemon with clax.dev (`clax share`), runtime
capabilities, key-bound agent tokens, a filtered per-reader mirror instead
of all-or-nothing read tokens, merge-back from forks, OAuth.

## 14. UI

Svelte 5 in `hosted/ui`, styled like the local shell (`web/shell`): CSS
tokens on `:root` with a dark mode, phone-width layout. Routes:

- `/` gallery: your artifacts, and those shared with you; each card shows
  the head version, who is working on it, open threads, forks count.
- `/a/:id[/v/:n]` the viewer: the content frame; a top bar (title, version
  picker, Fork, Share, Clone, presence chips of people and agents); a side
  panel of threads (new comment on the page, or select text in the frame
  to anchor a quote; each thread has replies, Send to ▾ agent picker,
  Resolve; withheld state shown to people as "held until <person>
  approves"); a version strip showing publisher and agent per version,
  `via git` versions marked; "Forked from <title> v<n>" and "Forks (n)".
- `/approvals` open approval requests with the four buttons.
- `/people` invite people, connect an agent (shows the MCP command once),
  your agent tokens, live agents.
- `/setup`, `/join/:code`, `/signin/:code`.

Time to usable: the viewer's first HTML carries the `ArtifactView` JSON in
`<script type="application/json" id="clax-view">` (so the app renders
without an API round trip), and the shown version's entry file URL in
`<meta name="clax-frame-src">` and a `<link rel="prefetch">`, so the content
loads while the app script starts; the app script creates the content
iframe with that URL.

## 15. Testing

- `hosted/test/` with `@cloudflare/vitest-plugin` (the Workers runtime): RepoStore (memory),
  DOs, rights, visibility, API, MCP tools.
- The pack writer and receive-pack client are tested in node against a real
  `git http-backend` served from a temporary directory, then `git fsck` and
  `git clone` of the result.
- `hosted/e2e/` Playwright against `wrangler dev` with `REPO_STORE=memory`:
  two people, two agents (driven through `/mcp` by the test), a held
  comment, approval, a conflict and retry, a fork, a guest's comment, git
  token rights, and a clone with git and jj from the `/git/*` remote (the
  push test is skipped: the memory store refuses pushes).
- `scripts/demo-agents.ts`: N concurrent scripted agents against a running
  instance, for the video.
- No sleeps standing in for conditions; no wall-clock budgets in
  functional tests.

## 16. Live collaboration

### 16.1 Live seats

Each artifact has live seats: 100 for people and 20 for agents (env
`LIVE_PEOPLE_SEATS`, `LIVE_AGENT_SEATS`). A signed-in person's live socket takes
a seat when it opens; the owner and people with a named share always get
one (their seats are reserved), guests share at most 20 of the people seats
(`LIVE_GUEST_SEATS`), and everyone else gets one while any is free, first
come first served. Anonymous link viewers take no seat: their socket
carries posted events only. A person without a socket may write while any
seat is free. Idle seats are released on the artifact's alarm. A seat is released when its socket closes or after
3 minutes without input from it (a pointer, a keystroke, a scroll or a
heartbeat the page sends while visible). One person with several tabs holds
one seat.

A person who gets no seat is a **spectator**: read-only (§7) with no live
socket. The viewer shows a banner: "Lots of people are here, so you're
viewing: comments and live activity are paused." with a **Join live**
button that appears when the spectator snapshot reports a free seat.

### 16.2 The spectator snapshot

`GET /api/artifacts/:id/snapshot` (people only; agents are never
spectators and are refused) answers the artifact as a `viewer` sees
it (an `ArtifactView` without per-person fields), plus `{viewing,
seats_free, guest_seats_free, built_at}`. Every spectator of an artifact gets the same answer, so it is
cached with the Cache API for 5 seconds (`s-maxage=5,
stale-while-revalidate=30`) keyed by artifact and head version; a
spectator's page polls it every 5 seconds while visible and swaps in a newer
version on its own. Content is served by commit as for everyone (§9), so a
new version costs one edge miss per location.

### 16.3 Live layers for seat holders

Live events go to seated people only; agents never receive drafts, typing
or cursors (agents see posted comments, filtered as §8 says).

- **Presence:** each seated person has a colour; the top bar shows chips
  for the people and agents that matter to the viewer (§16.5) and a count
  of everyone viewing, spectators included.
- **Typing and comment drafts:** while a seated commenter writes, the
  others see "Mia is typing…" on that thread (or on "New comment") and the
  draft text itself, debounced to about 150 ms, at most 2000 characters,
  cleared when posted or cancelled. Drafts are never stored.
- **Cursors and selections:** the injected content script reports the
  pointer and the text selection anchored to an element path and a text
  offset (not pixels), so they land correctly at every width, about 10 times
  a second while moving; it draws others' cursors and selections inside the
  frame, tinted by colour. Posted threads' quotes stay highlighted in the
  page.

### 16.4 Agents drafting live

The MCP tool `draft {id, files, note?}` sends an agent's work in progress:
changed files only, at most 1 MiB, at most two a second per agent. Seated
people see "clax-claude is drafting…" with **Show draft**; showing it
renders the draft in the frame from
`/c/:id/draft/<agent ID>/<token>/<path>` (an unguessable token per draft
revision; the content origin, the same CSP, `no-store`), with the elements
whose text differs from the shown version tinted in the agent's colour. The
Worker compares the draft with the base and marks changed elements; the
draft page carries those marks, never base text or hashes of it, compares
only against a base file the drafting agent may see (otherwise every element
is marked), and serves through the draft URL only base files that agent may
see. Drafts go to signed-in people's sockets (members and guests);
anonymous link viewers get none. The draft ends
when the agent publishes (the version lands as today), calls `draft` with
`files: {}`, or goes idle for 2 minutes. A draft is the agent's person's
words: people who can read the artifact see it; other agents never do.

### 16.5 Limits and cost

Each receiver gets full live detail (cursor, selection, draft) for at most
30 senders: their own agents, the people in the thread they have open, and
the most recently active; everyone else is a count. Each sender's updates
are coalesced to about 10 a second and sent in batches. Per-person rate
limits apply to comments, sends and publishes. With 100 seats the worst
case per artifact is about 30 × 10 × 100 messages a second, which one
artifact object handles; spectators cost one cached request per 5 seconds
each. Directory sharding for 100,000 people across all artifacts is later
work (§13).
