# Clax in Chrome: the comment overlay on any web page

Date: 2026-10-05
Status: implemented on branch chrome-overlay; one open question (§17)

This document designs a Chrome extension that puts Clax's comment overlay on
any web page, most often a developer's running dev server, and links the
threads made there to coding agents exactly as threads on published
artifacts are linked today. It builds on the main spec
(`2026-09-28-clax-design.md`, "the main spec" below) and on Echo
(`2026-10-01-echo-design.md`); where this document is silent, they hold.
Section numbers like §10 refer to the main spec.

## 1. Purpose

Today a person can comment only on pages an agent published to Clax. An
agent working on a web app shows its work on the app's own dev server
(`http://localhost:5173/`), not as an artifact. The extension lets the person
comment on that page itself: pick an element, a text selection or a drawn
area, write a comment, and send it to the agent, which receives it through
the channels it already uses (§10 tiers 1–5).

### Goals

- Comment on any `http:` or `https:` page in Chrome with the same picking
  rules as comment mode in the shell (element, range, area; Option widening).
- Every thread keeps a screenshot of what the person saw and a sanitized
  snapshot of the page, so it can be shown in context after the code
  changes.
- An agent links to a page by watching its URL; threads sent on that page
  reach the agent through the existing tiers. No session picker, no pairing
  per session.
- The side panel shows the page's threads live: replies, the working roster,
  Send to agent, resolve and reopen.
- Installation costs the person one "Load unpacked" until a Web Store listing
  exists, and nothing after it.
- Clax is on per tab: a tab the person has not turned it on in runs nothing
  of Clax, whatever site it shows and whatever permission Clax holds.

### Non-goals (this design)

- Firefox, Safari, and Chrome on Windows (§16 lists what is out of scope).
- Commenting on a snapshot in the shell. Snapshots are for reading threads
  in context; new comments are made on the live page.
- Syncing anything to a server other than the local daemon.

## 2. Decisions

The owner decided O1–O5 before this design; this document records them as
decided and works out their consequences. L1–L15 are this design's
decisions, each with its reason. On review the owner decided L6, L7, L8 and
L15 (2026-10-05); their rows say so. The owner changed O4 from per origin
to per tab (2026-10-06); its row says so.

| # | Decision | Reason |
|---|---|---|
| O1 | Pairing is Chrome native messaging. `clax init` (and `clax extension install`, §6.6) writes the extension to `~/.clax/extension` (unpacked; its ID is the one in effect, L15) and registers a native-messaging host manifest for Chrome, Chromium, Brave and Edge on macOS and Linux whose `allowed_origins` is exactly that extension's origin. The host is the `clax` binary (`clax native-host`), which returns the daemon URL and an extension-scoped credential, starting the daemon if needed. The daemon token never enters a web page's context; only the extension's service worker holds credentials. A later Web Store listing uses the same ID. `clax uninit` removes it; `clax doctor` reports it. | Owner decision: lowest-friction pairing. |
| O2 | Each comment on a live page saves a viewport screenshot (`chrome.tabs.captureVisibleTab`), stored as the thread's clip, and a sanitized DOM snapshot of the page, stored as a version of the page's artifact. | Owner decision: threads can always be shown in context after the code changes. |
| O3 | A live page is an artifact of a new kind, `live`, keyed by origin and path. The MCP `watch` tool accepts a page URL as well as an artifact ID or URL, creating the live page if needed; threads sent there reach the agent through the existing channels. No session picker. The plugin skills tell agents to watch their dev server's URL. | Owner decision: the agent link works exactly as now. |
| O4 | Any site, Chrome only, Manifest V3. Off by default; turned on **per tab** when the person clicks the toolbar icon (or uses the command or the context menu) in that tab: its side panel, its pins and comment mode, in that tab only. Other tabs, of the same site or any other, and new tabs show nothing until the person turns Clax on there. Within the tab it stays on through reloads, hot updates and navigations within its origin, and turns off when the tab navigates to another origin, when the person clicks the icon again (or "Turn off in this tab" in the panel), or when the tab closes. Chrome may keep the site's optional host permission, so a later click needs no prompt; holding it turns Clax on in no tab. Threads list in Chrome's side panel, reusing the shell's sidebar components; pins, highlights and the composer sit over the page inside a closed shadow root. The overlay reuses the bridge's comment-mode and anchoring code. Hot reloads and DOM changes re-resolve anchors (detached when gone). Firefox later. | Owner decision. Per tab rather than per origin: owner decision (2026-10-06), so that Clax appears only where the person asked for it, never in every tab of a site. |
| O5 | Everything else Clax does stays: realtime updates in the side panel (working roster, agent replies), Send to agent, resolve and reopen, "addressed" adapted for live pages, and the gallery shows live pages beside artifacts. | Owner decision. |
| L1 | The live-page key is the URL's origin (scheme, lowercased host, non-default port) and path. The query string and a plain fragment are not part of the key. A hash route (`#/…` or `#!/…`) is not part of the key either. The query (less `utm_*`, `fbclid`, `gclid`) and the hash route form the thread's **route**, stored on its anchor (§5.2). | A query or hash route is a view of one page's code: keying by it would split one component's threads over many artifacts and mint an artifact per cache-buster. Recording the route on each thread keeps it shown where it was made. Path-routed and hash-routed apps then behave alike: one artifact per path, threads filtered by route. |
| L2 | A `watch` on a page URL is a **scope watch**: it covers that page and every live page of the same origin whose path is the watched path or below it (`/` covers the whole origin). Scope watches are materialized as ordinary `watches` rows (marked `source = 'scope'`) on every covered live page, now and as new ones are created. | An agent watching `http://localhost:5173/` must also hear about threads on `/settings`. Materializing keeps feedback targeting, participants, retargeting and every tier unchanged, since they read `watches`. |
| L3 | A live page created without a snapshot (by `watch`) gets a placeholder version 1, an HTML page saying no snapshot exists yet. Every later version is a snapshot. | Every query of live artifacts, the doctor's cleanup and the shell assume at least one version; a placeholder keeps that invariant instead of special-casing a dozen queries. |
| L4 | A snapshot becomes a new version only when its `index.html` differs from the current version's (byte-identical snapshots reuse the current version). The screenshot is the thread's clip, never a version file. | Versions stay a timeline of what the page looked like, not one per comment; the clip already has a home per thread (§9 "Clips"). |
| L5 | The extension calls the daemon's existing viewer routes through an **extension gateway**: a middleware that admits a request carrying the extension's origin and a valid extension credential, checks it against an allowlist of routes and live-page artifacts, and hands it to the existing handler as the owner identity (L6). | One code path for comments, sends, resolves and the stream, whether they come from the shell or the extension; the extension can reach nothing else. |
| L6 | The extension acts as the **owner identity**: the one viewer identity Clax gives its owner, shared by the person's browsers on this machine, the CLI with the token, and the extension (§2.1). A live extension credential maps to the owner identity through the owner identity's server-side hook, in the one place that maps credential kinds to it; the extension has no `viewers` row of its own. Its looked-at and seen marks, its name, its presence and the comments, sends and resolves it makes are the owner's. The side panel asks for a name only while the owner has none. | Owner decision (2026-10-05): one person, one identity, whichever client they use. The side panel and the shell then share looked-at and seen marks, and a thread the person read in one is not new in the other. The extension needs no cookie: the gateway, not a browser cookie, says who it is. |
| L7 | The composer over the page is an extension page (`composer.html`) in an iframe inside the closed shadow root. Thread text is shown only in the side panel; on the page, pins carry only a number (and, for a thread of another page of the site, that page's path). Anchors do reach the overlay, and since 2026-10-06 (site-wide pins) those of the site's other pages too: their quote, prefix and suffix are text of *other* pages of the origin, which the overlay of this page holds in its isolated world and closed shadow root (owner decision 2026-10-06: pins for any thread of the site whose anchor resolves here); comments, authors and agents still never reach it. | Owner decision (2026-10-05), as proposed. Key events in a closed shadow root are still dispatched through the page's window, so page scripts could read every key typed into an in-page textarea. Text typed into an extension-origin frame never reaches the page. The frame is shown and focused only once the worker confirms the composer page connected (§9.4), so a page that navigates the frame never gets it shown. |
| L8 | Comment mode is entered only by a gesture that grants `activeTab`: the toolbar icon, the keyboard command (Alt+Shift+C) or the page's context menu entry. The side panel's Comment button works while the tab already holds `activeTab` and otherwise says how to start. A plain key (such as C) is never taken from the page. | Owner decision (2026-10-05), as proposed. `captureVisibleTab` needs `activeTab` or `<all_urls>`; per-origin host permissions are not enough. `<all_urls>` would ask for every site at install. |
| L9 | Snapshots are sanitized in the extension (§8.2) and served by the daemon with a second Content-Security-Policy that lets only the daemon's own `/_clax/` scripts run. Comment mode is off in the shell for live pages. | Page content is hostile. The client sanitizer removes secrets and scripts; the policy guarantees nothing the page wrote can run even if the sanitizer misses something, without a server-side HTML parser. |
| L10 | Live pages are visible only to loopback peers and requests with the token. A daemon bound to the LAN serves LAN viewers its artifacts as today, and never its live pages. | Agents publish artifacts for people to see; live-page snapshots are of whatever the person browses. They must not reach the LAN. |
| L11 | "Addressed" on a live page: the agent marks a thread addressed with `comments_reply` and `addressed: true` (or resolves it), which records a **pending address**. The next snapshot of the page links every pending address to its version, so the thread reads "addressed in vN" with the page as it looked after the fix. The extension takes that snapshot itself (DOM only, no screenshot) when the page is open, visible and has pending addresses, once the DOM has been quiet for 1 s. Ruling 2026-10-05: the snapshot names the pending threads it covers. Both routes that carry a snapshot (`POST /api/live/threads` and `POST /api/live/snapshots`) take `pending`: the extension reads the pending thread IDs from the page state before it serializes the page and sends them with the snapshot; only those still pending are linked, in the transaction that stores the version, and an address made after the serialization waits for the next snapshot. | There is no publish on a live page. Linking to the snapshot that shows the fix keeps Echo's meaning of "addressed in vN" (a version that handled the thread) and works for hot reloads, which change the page without a version. |
| L12 | `publish` refuses a live page (400 `live_page`). `read`, `list`, `delete`, `comments_*`, `working`, `watch` and `wait_for_feedback` work on it. | Snapshots come from the browser; an agent publishing a page under a live page's ID would break the timeline. |
| L13 | The extension's files are embedded in the `clax` binary and written by `clax extension install`, which `clax init` runs. The extension compares its manifest version with the daemon's on pairing and calls `chrome.runtime.reload()` once per new version, which reloads an unpacked extension from disk. | After an upgrade the person never visits `chrome://extensions` again. |
| L14 | The native host is launched through a script in `~/.clax/extension/host/` that execs a copy of the plugins' wrapper (`ensure-clax.sh exec native-host`), with `CLAX_HOME` fixed to the home that installed it. | A host manifest names one absolute path with no arguments. The wrapper already finds the right binary (`CLAX_BIN`, the `bin` setting, the pinned release) and survives upgrades that remove old version directories. |
| L15 | No key is needed to build, install, test or use the extension locally. Without a `key` in `manifest.json`, Chromium derives an unpacked extension's ID from its canonicalized absolute install path (the first 128 bits of the path's SHA-256, each nibble written as a letter `a`–`p`). The **ID in effect** for a Clax home is therefore: the ID derived from the committed public key when `web/extension/key/key.pub.b64` exists, else the ID derived from `<home>/extension`. `clax extension install`, the native host's origin check, the credential routes and the gateway all use the ID in effect, computed once per home (`AppState`), and a test pins both derivations (path `/Users/alex/.clax/extension` → `bhhldgpcjhfhmcfjjnelbbdcefnocaln`; the e2e checks Chromium's own ID for the loaded extension against the derivation). The committed public key is optional and only fixes the ID ahead of a Web Store listing. The private key never enters Clax, the repository, the build or CI: it lives only in the owner's 1Password, referenced by `CLAX_EXTENSION_KEY_REF` (an `op://` reference), and is read only by the owner-run, approval-gated signing scripts (§6.7). | Owner decision (2026-10-05): test locally with no key, and sign through 1Password. Nothing in Tasks 1–16 waits on the owner. Adding the public key later changes the ID once (one more Load unpacked for each person, and a fresh native-host registration, which `clax extension install` writes). |

### 2.1 Depends on: the owner identity

The owner identity is built separately (branch `cli-comments`) and lands on
main before this design is implemented. This design uses exactly two things
from it, under whatever names it gives them:

1. **The owner viewer**: one `viewers` row (public ID, display name) that
   is the owner wherever they act, with a way for the daemon to resolve it
   (creating it on first use if that work does so).
2. **The hook**: the one server-side place that maps an authenticated
   credential kind to the owner identity, so the viewer routes
   (`/api/viewers/me`, looked-at, presence, comments, sends, resolves,
   deletes, the stream) treat a request as the owner's. The gateway (L5)
   registers the extension credential there as one more kind that maps to
   the owner. If the hook works by presenting the owner viewer to the
   existing handlers as the viewer cookie does, the gateway sets that; if it
   is a principal the handlers read, the gateway sets that. Either way no
   handler learns about the extension.

The owner identity's level on the viewer routes is whatever that work
gives the owner; the gateway's allowlist (§9.2) bounds what the extension
can reach regardless, and the gateway never presents the daemon token.

## 3. User flows

### 3.1 First run

1. The person installs a Clax plugin and runs `clax init` (or `/clax:extension`
   in Claude Code, or `clax extension install`; §6.6). It writes
   `~/.clax/extension/`, registers the native host with every supported
   browser it finds, and prints: "Chrome: open chrome://extensions, turn on
   Developer mode, choose Load unpacked, and pick ~/.clax/extension (once)."
2. They load it. The Clax icon appears in the toolbar (pin it to keep it
   visible).
3. On their dev server tab they click the icon. Clax opens the side panel
   for that tab, asks Chrome for access to `http://localhost:5173` ("Allow
   Clax to read and change site data on localhost:5173?"; once allowed,
   Chrome asks no more), injects the overlay and turns comment mode on. Clax
   is now on in that tab only: another tab of the dev server, or a new one,
   shows nothing until the person clicks the icon there. A reload or a
   navigation within the origin keeps it on (the overlay comes back, comment
   mode off); a navigation to another origin turns it off. The service worker pairs with the daemon on its first
   need (native host, under 300 ms with a running daemon; it starts one if
   none runs).
4. If the owner has no name yet (none set in the shell or the CLI), the side
   panel asks for it once ("Your name"); the name is the owner's
   everywhere.

### 3.2 Commenting

1. Comment mode is on: hovering outlines targets exactly as in the shell.
   Click picks an element, a drag selection picks a range, a drag over no text
   (or with Shift) draws an area, Option widens.
2. At the pick, the overlay hides its own drawing, the service worker takes a
   screenshot of the visible tab, draws the pick's outline on it, and the
   composer opens beside the target, focused. The page's snapshot is
   serialized while the person types.
3. Post creates the thread: the daemon finds or creates the live page,
   stores the snapshot as a version when it changed, the screenshot as the
   clip, and the thread on that version. Comment mode comes back on for the
   next pick, as in the shell.
4. The side panel shows the thread (numbered like its pin). Send to <agent>
   sends it; `@agent` in the text sends it too.

### 3.3 The agent

1. In its session the agent is told by the skill to watch the dev server it
   started: `watch({url_or_id: "http://localhost:5173/"})`. The result names
   the live page, the Clax view URL, and the scope it covers
   (`http://localhost:5173/*`).
2. Threads sent on any page of that origin reach the session through tiers
   1–5, with the page URL, the route, the clip and the snapshot path in the
   payload (§9.3).
3. The agent fixes the code; the dev server hot-reloads. The overlay
   re-resolves every pin: a changed element keeps its pin and gets
   `outdated`; a removed one goes to Detached.
4. The agent replies with `addressed: true` ("Fixed: the button now wraps").
   The page is open, so the extension takes a snapshot once the DOM is quiet;
   the thread reads "claude · addressed in v4". The person looks, then
   resolves (or reopens).

### 3.4 The whole site (owner decision 2026-10-06)

The side panel lists "This page" first and, below, "Elsewhere on this site":
every thread of the origin's other live pages, grouped under each page's
path (a merged page under its pattern, its threads marked with the path
they were made at), each group with its open, addressed and resolved
counts, newest activity first and collapsible. Clicking such a thread
navigates this tab to its page (`page_url`; a same-origin navigation, so
Clax stays on), and the overlay scrolls to and highlights it once its
anchor resolves there. A thread of another page whose anchor resolves on
the current screen is pinned here too, outlined and marked "from
/other/path"; one not found stays listed under its own page only.

A status filter (Open, Addressed, Resolved, All) and a search (comment
text, authors, quote, page path) narrow both lists, on the listing in the
panel. "Move…" on a card, or "Move the selected thread to another page…"
for this page's selected thread, re-files it under a page of the site or
the current page. "Merge pages" takes a pattern (`/users/:id`), says why
the daemon would refuse it or previews which pages it matches, and merges
in batches with progress; the rules in force are listed with "Un-merge",
which un-merges the same way. The listing follows the `site:` topic live: a
moved thread's card moves to its new group.

### 3.5 Reviewing later

The gallery shows the live page beside artifacts, marked "Live" with its
page URL. Opening it shows the latest snapshot with its pins, the version
menu listing snapshots, and every thread as in any artifact, with an "Open
page" link to the live URL.

### 3.6 Turning it off

A second click on the icon, or the side panel's "Turn off in this tab",
turns Clax off in that tab: the overlay stops in place (no reload), and the
tab's side panel is disabled. Closing the tab or navigating it to another
origin does the same. Threads stay. Chrome keeps the origin's permission
(the person removes it in Chrome's own site settings).
`clax uninit` removes the native host registration and `~/.clax/extension/`,
revokes every extension credential, and prints how to remove the unpacked
extension from Chrome.

## 4. Architecture

```
  Chrome (one profile)
  ┌───────────────────────────────────────────────────────────────────────────┐
  │  tab: http://localhost:5173/settings            side panel (extension page)│
  │  ┌──────────────────────────────────────┐      ┌────────────────────────┐ │
  │  │ page (main world)                    │      │ sidepanel.html         │ │
  │  │  ┌────────────────────────────────┐  │      │ Svelte: Sidebar,       │ │
  │  │  │ isolated world (tabs Clax is on│  │      │ ThreadCard, Roster…    │ │
  │  │  │ in): overlay.js, injected      │  │      └──────────▲─────────────┘ │
  │  │  │   CommentMode, anchors, pins   │  │                 │ port          │
  │  │  │   snapshot serializer          │  │                 │               │
  │  │  │   <clax-overlay> closed shadow │  │      ┌──────────┴─────────────┐ │
  │  │  │     └ iframe composer.html ────┼──┼─port─►  service worker (sw.js)│ │
  │  │  └──────────────┬─────────────────┘  │      │  credential, pairing   │ │
  │  └─────────────────┼────────────────────┘      │  API client + stream   │ │
  │                    └──── runtime port ─────────►  captureVisibleTab     │ │
  │                                                 └──┬───────────┬────────┘ │
  └────────────────────────────────────────────────────┼───────────┼──────────┘
                     sendNativeMessage (stdio, 1 msg)  │           │ fetch, CORS
                                                       ▼           │ Origin: chrome-extension://<ID>
                                ┌──────────────────────────┐       │ Authorization: Clax-Extension <cred>
                                │ clax native-host         │       ▼
                                │  (via host/launch.sh →   │  ┌──────────────────────────────────┐
                                │   ensure-clax.sh)        │──►  clax serve (daemon)              │
                                │  ensures daemon, mints   │  │  extension gateway (L5)          │
                                │  credential with token   │  │  /api/live/*, viewer routes,     │
                                └──────────────────────────┘  │  /api/stream (live-only caller)  │
                                                              │  SQLite: live_pages, live_watches│
            agents: clax mcp ── watch(url) ──────────────────►│  live_pending, extension_*       │
                                                              └──────────────────────────────────┘
```

Trust boundaries: the page's main world is hostile. The isolated world shares
its DOM but not its JavaScript; it holds no credential. The composer frame,
the side panel and the service worker are extension pages, out of the page's
reach. Only the service worker holds the credential and talks to the daemon.

## 5. Data model

### 5.1 Migration 16: live pages

```sql
ALTER TABLE artifacts ADD COLUMN kind TEXT NOT NULL DEFAULT 'html'
    CHECK (kind IN ('html', 'live'));
CREATE TABLE live_pages (
    artifact_id TEXT PRIMARY KEY REFERENCES artifacts(id),
    origin TEXT NOT NULL,
    path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (origin, path)
);
CREATE TABLE live_watches (
    session_id TEXT NOT NULL REFERENCES sessions(id),
    origin TEXT NOT NULL,
    path TEXT NOT NULL,
    replies_armed INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    PRIMARY KEY (session_id, origin, path)
);
ALTER TABLE watches ADD COLUMN source TEXT NOT NULL DEFAULT 'direct'
    CHECK (source IN ('direct', 'scope'));
CREATE TABLE live_pending (
    artifact_id TEXT NOT NULL,
    thread_id TEXT NOT NULL REFERENCES threads(id),
    source TEXT NOT NULL CHECK (source IN ('explicit', 'resolve')),
    harness TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (artifact_id, thread_id)
);
CREATE INDEX live_pending_by_thread ON live_pending(thread_id);
CREATE INDEX live_watches_by_origin ON live_watches(origin, path);
CREATE TABLE live_picks (
    artifact_id TEXT NOT NULL,
    pick_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (artifact_id, pick_id)
);
CREATE INDEX live_picks_by_time ON live_picks(created_at);
```

- `artifacts.kind`: every existing artifact is `html`. Artifact views gain
  `kind` and, for `live`, `live: {origin, path, page_url}` (`page_url` is
  `origin + path`).
- `live_pages`: the key (§7). Deleting a live artifact deletes its row, so a
  later comment on the same URL starts a new live page.
- `live_watches`: scope watches (L2). A session's rows go when the session
  ends, with its watches.
- `watches.source`: `scope` rows were made by a scope watch and are removed
  with it; a direct `watch` on the artifact turns a row `direct`, which a
  scope removal keeps.
- `live_pending`: pending addresses (L11). Linking moves a row into
  `version_threads` (with its `source`) and deletes it; deleting the thread
  deletes it.
- `live_picks`: the thread each recent pick made, so a repeated
  `POST /api/live/threads` for the same pick makes no second thread (§9.2).
  Rows older than an hour are dropped when a new one is written; a purged
  page's rows go with it. No foreign key: a deleted thread leaves its row,
  which no longer matches and is replaced by the pick's next thread.

### 5.2 Anchors gain `route`

`Anchor` (Rust `clax_core::Anchor`, TS `protocol.ts`) gains `route`: a string
of at most 512 bytes, no control characters, U+2028 or U+2029, starting with
`?` or `#`, or absent. The daemon sets it on threads of live pages from the
page URL the thread was posted with (the client's value is ignored) and
refuses it on other artifacts (`invalid_anchor`). Its summary prefix is
`<route> › ` when present, as `<file> › ` is today.

### 5.3 Migration 17: extension credentials

```sql
CREATE TABLE extension_credentials (
    id TEXT PRIMARY KEY,
    extension_id TEXT NOT NULL,
    secret_sha256 TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_used_at TEXT NOT NULL,
    revoked_at TEXT
);
CREATE INDEX extension_credentials_by_extension ON extension_credentials(extension_id, created_at);
```

A credential is `cxe_` and 43 base64url characters (32 random bytes). Only
its SHA-256 is stored. A credential is live while not revoked and used within
30 days; `last_used_at` is written at most once an hour per credential. At
most 8 live credentials exist per extension ID: minting a ninth revokes the
oldest. The daemon keeps an in-memory map of live credential hashes to
their extension ID, loaded on start and updated on mint and revoke. A
credential carries no viewer: every live one is the owner identity (L6).

### 5.4 Files

```
~/.clax/
  extension/                  written by `clax extension install`
    manifest.json, sw.js, overlay.js, composer.html, composer.js,
    sidepanel.html, sidepanel.js, icons/…
    host/launch.sh            the native host's path (L14)
    host/ensure-clax.sh       a copy of the plugins' wrapper
    installed.json            {version, hosts: [paths written]}
  artifacts/<aid>/versions/<n>/index.html    a live page's snapshot (or placeholder)
  artifacts/<aid>/clips/<tid>.png            a thread's screenshot
```

Native host manifests (`dev.empathic.clax.json`), written only where the
browser's profile directory exists:

| Browser | macOS (`~/Library/Application Support/…`) | Linux (`~/.config/…`) |
|---|---|---|
| Chrome | `Google/Chrome/NativeMessagingHosts` | `google-chrome/NativeMessagingHosts` |
| Chrome Beta | `Google/Chrome Beta/NativeMessagingHosts` | `google-chrome-beta/NativeMessagingHosts` |
| Chrome Dev / unstable | `Google/Chrome Dev/NativeMessagingHosts` | `google-chrome-unstable/NativeMessagingHosts` |
| Chrome Canary | `Google/Chrome Canary/NativeMessagingHosts` | — |
| Chromium | `Chromium/NativeMessagingHosts` | `chromium/NativeMessagingHosts` |
| Brave | `BraveSoftware/Brave-Browser/NativeMessagingHosts` | `BraveSoftware/Brave-Browser/NativeMessagingHosts` |
| Edge | `Microsoft Edge/NativeMessagingHosts` | `microsoft-edge/NativeMessagingHosts` |

`CLAX_NATIVE_HOST_DIRS` (colon-separated) replaces this table, for tests and
unusual installs.

```json
{
  "name": "dev.empathic.clax",
  "description": "Clax: pairs the Clax extension with the local Clax daemon",
  "path": "/Users/alex/.clax/extension/host/launch.sh",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://<the ID in effect, L15>/"]
}
```

## 6. Components

### 6.1 Daemon

- `clax_core::live`: URL normalization into `PageKey {origin, path}` and
  `route` (§7); store functions to find, create and snapshot live pages,
  scope watches and pending addresses.
- `/api/live/*` routes (§9.2) and the extension gateway (L5, §10.3).
- Credential mint and status routes (token only).
- Serving: a live page's HTML versions carry the snapshot policy (§8.4).
  Live pages are hidden from non-loopback callers without the token (L10).

### 6.2 MCP shim and tools

- `watch`, `comments_read`, `comments_reply`, `comments_resolve`, `working`
  and `wait_for_feedback` accept a page URL in `url_or_id`. `watch` with a
  page URL makes a scope watch (creating the live page); the others resolve a
  page URL to its live page (`invalid_id` when none exists).
- A URL is an artifact reference only when it names the daemon (its origin,
  any `<id>.localhost` on the daemon's port, or another host on the daemon's
  port) and has the `/a/<id>` or `/c/<id>/v/<n>` form; any other `http(s)`
  URL is a page URL. (Today any URL with an `/a/<id>` segment is taken as an
  artifact reference, whatever its host.)
- `comments_reply` gains `addressed` (boolean, default false), accepted only
  on live pages.
- The payload and `comments_read` carry the page URL and the snapshot path
  (§9.3).

### 6.3 CLI

- `clax native-host`: the native messaging host (§9.1).
- `clax extension install | uninstall | status`: writes or removes the
  extension's files and the host manifests, and reports them; `--json` as
  every command. `clax init` runs `install`, `clax uninit` runs `uninstall`
  and revokes credentials, and `clax doctor` includes `status`.
- The Claude Code plugin gains `/clax:extension` (runs `clax extension
  install` through the wrapper and prints the Load unpacked line), for people
  who installed only the plugin, since plugins never run `clax init`.

### 6.4 The extension (`web/extension`, built to `web/dist-extension`)

| File | Role | Size target (gzip) |
|---|---|---|
| `manifest.json` | MV3 manifest; `key` only when a public key is committed (L15) | — |
| `sw.js` | Service worker (ES module): pairing, credential, API client, stream hub, per-tab enablement, screenshot, pick state | 24 KiB |
| `overlay.js` | Injected into the isolated world of each tab Clax is on, once per document: CommentMode, anchors, pins, re-resolution, snapshot serializer, composer frame host | 30 KiB |
| `composer.html/js` | The composer (Svelte, reusing `Composer.svelte`) | 25 KiB |
| `sidepanel.html/js` | The side panel (Svelte, reusing `Sidebar.svelte`, `ThreadCard`, `SendButton`, `Roster`, `WorkingStrip`, `ViewerName`) | 60 KiB |

Manifest essentials:

```json
{
  "manifest_version": 3,
  "name": "Clax",
  "version": "<clax version, numeric part>",
  "key": "<base64 SPKI public key, only when web/extension/key/key.pub.b64 is committed (L15)>",
  "minimum_chrome_version": "116",
  "action": {"default_title": "Comment with Clax"},
  "background": {"service_worker": "sw.js", "type": "module"},
  "permissions": ["activeTab", "scripting", "sidePanel", "storage", "nativeMessaging", "contextMenus"],
  "optional_host_permissions": ["http://*/*", "https://*/*"],
  "commands": {"comment": {"suggested_key": {"default": "Alt+Shift+C"}, "description": "Comment on this page"}},
  "web_accessible_resources": [{"resources": ["composer.html"], "matches": ["http://*/*", "https://*/*"], "use_dynamic_url": true}],
  "content_security_policy": {"extension_pages": "script-src 'self'; object-src 'none'; connect-src 'self' http://localhost:* http://127.0.0.1:*; img-src 'self' blob: data: http://localhost:* http://127.0.0.1:*"}
}
```

No `host_permissions`, no `content_scripts` and no `side_panel.default_path`
are declared, and the worker registers no content script: nothing runs on
any page until the person turns Clax on in its tab.

Clax on per tab (O4, 2026-10-06). The worker keeps each tab's state, with
the origin Clax is on for there, in `chrome.storage.session` (cleared when
the browser quits), so a restarted worker picks every on tab up again.

- **Side panel.** Disabled globally (`chrome.sidePanel.setOptions({enabled:
  false})` at each start) and enabled per tab (`setOptions({tabId, path:
  "sidepanel.html?tab=<tabId>", enabled: true})`), so Chrome hides it when
  the person switches to another tab and shows it again on the way back;
  the worker enables it again for an on tab as it comes to the front and at
  each start. Each tab's panel page is pinned to its tab (`?tab=`): it acts
  on that tab wherever the tab goes, another window included, and reports
  the owner's presence for the window the tab is in now. `turn-off` names
  the panel's tab, and the worker refuses it for another.
- **The site's threads** (§7.1, owner decision 2026-10-06). The worker
  follows `site:<origin>` (one hub client per origin, `site:<origin>`) for
  every origin Clax is on for in a tab the daemon answered a lookup for,
  and keeps the origin's listing (`GET /api/live/site`, fetched when the
  topic goes live or resyncs, when a delta does not add up — a page the
  listing lacks, comments that do not match their count — and after every
  rule batch; deltas heard during a fetch are applied to its answer).
  `thread_moved` moves the thread's card, comments and all, to the page it
  went to (the listing is fetched again when that page is new); the
  `thread` that follows completes it. Each panel of a tab of the origin
  hears `site {site}` when it watches and on every change. The overlay's
  `state` carries, after the page's own threads, the site's open threads of
  other pages as `{id, status, anchor, addressed_pending: false, from}`
  (`from`: the path they were made at), at most 200 (`MAX_FAR`), the
  newest; the overlay resolves the page's own threads first, then those on
  any route, and outlines their pins, labelled "from <path>". The listing is
  fetched at most once at a time per origin, with one more queued however
  many deltas fail to apply meanwhile; a failed fetch is retried after 1 s,
  doubling to 30 s. A replayed `thread_moved` already applied changes
  nothing. The panel sends
  `open-thread {threadId}` (the worker navigates the tab to the thread's
  `page_url`, of the tab's origin only, and scrolls to it once the overlay
  finds it), `move {threadId, pageUrl}` (of the tab's origin only), and
  `rule {req, origin, pattern}` / `unrule {req, origin, ruleId}`, one batch
  each, answered `step {req, moved, remaining}` or `failed {…, req}` (every
  request is answered: `no_tab`, or `page_changed` when `origin` is not the
  one Clax is on for in the panel's tab, so a panel that turned to another
  tab cannot write to its site). The panel asks the person to confirm a
  merge or an un-merge (naming the pages and threads), repeats the batch
  while `remaining` is above 0, and stops, saying how many are left, when a
  batch moves none, when `remaining` does not fall, after 1000 batches,
  when the panel shows another site, or when the worker does not answer
  within 2 minutes. Grouping, counts, the filter, the
  search, the pattern check and the merge preview run in the panel; the
  filter and the collapsed groups are kept per origin in
  `chrome.storage.local` (`site-prefs:<origin>`; a convenience only).
- **Gestures.** The icon's handler sets the tab's options and calls
  `chrome.sidePanel.open({tabId})` synchronously, inside the gesture, then
  asks for the origin's permission, before any `await`. The icon in a tab
  Clax is on turns it off: `setOptions({tabId, enabled: false})`, the
  overlay told `off` (it stops in place) both in the document that last
  wrote and in the tab's top frame, the tab's pick and state dropped. The
  command and the context menu turn Clax on likewise (the panel enabled,
  not opened) and, in a tab it is on in, flip comment mode. A worker that
  has not read its tabs back when the icon is clicked cannot tell whether
  the tab is on, and cannot wait (`sidePanel.open` must be called within the
  gesture): it opens the panel, and closes it again once it finds the tab
  was on. When the person refuses the permission, the panel says Clax will
  turn off in the tab when the page reloads.
- **Navigation.** On each `tabs.onUpdated` of an on tab that carries a URL
  or a load status, the worker reads the tab's URL: another origin turns
  Clax off there, as does any such update whose URL the extension may not
  read (an origin it holds no permission for), at `loading` already, so a
  bounce back cannot keep it on. Within the origin, the new document gets
  the overlay once loaded, and a URL change with no load (an activated
  prerender, a page restored from the back/forward cache, an in-page
  navigation) gets it if its document lacks one. `tabs.onReplaced` turns
  the replaced tab off; the new tab starts off.
- **Injection.** The worker probes the tab's top frame
  (`executeScript({target: {tabId, frameIds: [0]}})`), which answers whether
  the document has a live overlay and its origin, with the document's
  `documentId`; it injects only when that origin is the one Clax is on for
  in the tab, and only into that document (`target: {tabId, documentIds}`),
  so a navigation meanwhile gets nothing. After each await it checks again
  that the tab is still on for that origin.
- **Refusal.** The worker answers an overlay message it does not admit
  with `{off: true}`, and the overlay stops: one in a page restored from the
  back/forward cache after Clax turned off (the `off` sent then went
  unheard), or one that landed in a document Clax was never on for. A page
  restored from the back/forward cache (`pageshow` with `persisted`) sends
  `route` at once, so the check happens at the restore.

Loaders an earlier build registered per origin are unregistered at each
start. No `externally_connectable` is
declared (an empty one only draws a load warning), so no web page can message
the extension; and no part of it listens for other extensions' messages
(`onMessageExternal`, `onConnectExternal`), so theirs are dropped. A test
holds both.

### 6.5 Shell

- Gallery cards for live pages carry a "Live" chip and the page URL as the
  description line.
- The artifact view of a live page: the title line reads "Live page ·
  localhost:5173/settings" with an "Open page" link; Comment is disabled with
  the hint "Comment on the live page with the Clax extension"; the version
  menu and the version moment call versions snapshots ("snapshot v4"); a
  thread with a pending address shows "<agent> addressed it · waiting for a
  snapshot" in its history line.

### 6.6 Install

`clax extension install` writes the embedded files to
`~/.clax/extension/` (removing files a previous version had that this one
lacks), writes `host/ensure-clax.sh` (the wrapper this binary carries) and
`host/launch.sh`:

```sh
#!/bin/sh
# Launches the Clax native messaging host for the Clax Chrome extension.
CLAX_HOME='/Users/alex/.clax'
export CLAX_HOME
exec '/Users/alex/.clax/extension/host/ensure-clax.sh' exec native-host "$@"
```

then a host manifest in each browser directory of §5.4 that exists, and
`installed.json`. It never touches a browser directory that does not exist.
`uninstall` removes exactly the manifests `installed.json` lists whose `path`
is this home's `launch.sh`, then the directory. `status` reports, per
browser, `missing`, `installed`, or `stale` (another path or origin), and
whether the files match this binary.

### 6.7 Distribution and signing

Until a Web Store listing exists, the extension is distributed only as the
unpacked files `clax extension install` writes (§6.6), under the ID in
effect (L15). No key is involved.

Signing is local, owner-run and approval-gated; CI never signs. The private
key lives only in the owner's 1Password; `CLAX_EXTENSION_KEY_REF` holds its
full `op://` reference, so no vault or item name is written anywhere in the
repository.

- `scripts/extension-pubkey.sh` runs
  `op read "$CLAX_EXTENSION_KEY_REF" | openssl rsa -pubout -outform DER | base64`
  and writes `web/extension/key/key.pub.b64` (one line), which is committed. The
  private key passes only through the pipe; with the 1Password app's CLI
  integration, the app asks the owner to approve each read (a
  service-account token does not ask). The key is built into the binary,
  so the owner rebuilds and reinstalls clax before `clax init`.
- `scripts/pack-extension.sh` builds the Web Store upload zip from the
  release build and, with `--crx`, a signed `.crx` (Chromium's
  `--pack-extension-key`). It reads the private key from 1Password into a
  mode-0600 temporary file removed on exit (trap), never into the
  repository. The zip's `manifest.json` is the build's without `key`, which
  the Web Store refuses ("key field is not allowed in manifest"); the `.crx`
  keeps it. The private key goes into the zip, at its root as `key.pem`,
  only for the listing's first upload (`--first-upload`), which is what
  keeps the listed ID equal to the committed public key's; the Web Store
  re-signs every later release itself. Until that zip is complete, an exit
  or interrupt removes its directory too.

The scripts' tests use a fake `op` on `PATH` and a throwaway key generated in
the test; no gate or CI job calls 1Password. The upload itself is the
owner's manual step, and `docs/verification.md` lists it among what only the
owner does. Clax, its agents and its builds never read the private key.

## 7. Live-page identity

The daemon is the only normalizer; the extension sends `location.href` and
uses the `route` the daemon returns.

1. Parse with the WHATWG URL rules (`url` crate). Only `http` and `https`;
   otherwise `unsupported_url`. At most 4096 bytes; otherwise `invalid_url`.
2. Origin: scheme, lowercased host (IDNA as parsed), port when not the
   scheme's default. Credentials are dropped.
3. Path: the parsed path (dot segments resolved, percent-encoding as the
   parser leaves it); a trailing slash is kept (`/docs` and `/docs/` are
   different pages, as many servers treat them).
4. Route: the query with `utm_*`, `fbclid` and `gclid` parameters removed
   (order kept; `?` dropped when nothing is left), then the fragment when it
   starts with `/` or `!/` (a hash route). Any other fragment is dropped. The
   route is cut to 512 bytes at a character boundary.
5. The daemon's own origin (any of its local names and port) is refused
   (`own_origin`): Clax's pages have their own comment mode.

Examples: `http://LOCALHOST:5173/settings?tab=billing&utm_source=x#top` →
origin `http://localhost:5173`, path `/settings`, route `?tab=billing`;
`http://localhost:3000/#/users/7` → path `/`, route `#/users/7`.

The overlay shows a pin for a thread only when the thread's route equals the
current route; the side panel lists other routes' threads as "on
?tab=billing", and clicking one navigates the tab there. A route-less thread
shows on the route-less view.

Known consequence: a port reused by another project's dev server shares its
live pages. The person deletes the live page from the gallery to start over.

### 7.1 Site-wide threads, moving and merging pages

Owner decision, 2026-10-06: the side panel shows, besides the current
page's threads, "Elsewhere on this site": every thread of every live page of
the same origin, grouped by page; clicking one navigates the tab there; pins
show for any thread of the site whose anchor resolves on the current
screen; the panel filters by status, searches, moves a thread to another
page, and merges pages (declares that paths are one page, so their threads
group together from then on). The daemon provides the data and the writes;
the panel's search and filters run on the listing in the client.

1. **Listing.** `GET /api/live/site?origin=` answers every live page of the
   origin that has threads, newest activity first, each with its page view,
   a summary (`open`, `addressed`, `resolved` counts and `last_activity`)
   and its thread views (resolved ones too), plus the origin's merge rules.
   A live page's thread view carries `page_path` (the path it was made at)
   and `page_url` (origin, `page_path` and route: the URL the panel opens),
   and `moves`. The reads are index-bound: pages by `live_pages`' origin
   index, threads by `threads_by_artifact`, rules by their origin index,
   moves by `thread_moves_by_thread`.
2. **Realtime.** The stream topic `site:<origin>` (the normalized origin)
   carries every live page's `artifact:` events for the origin, pages made
   later included; only streams that may see live pages take it (L10), and
   only an owner's (the listing's read rule); the extension's live-only
   stream does.
3. **Moving.** `POST /api/live/threads/<tid>/move {page_url}` (the owner:
   the token or the extension's credential) re-files a live page's thread
   under the live page `page_url` names, of the same origin only
   (`cross_origin` otherwise), made through `LiveIds::ensure_page` when
   missing, at `page_url`'s route. The thread keeps its ID, comments, sends,
   feedback and status; its clip, pending address and pick move with it.
   - **Snapshots.** The version it was made on and each version that
     addressed it become versions of the new page, so the invariant "a
     thread's version and its address links are versions of its own page"
     holds and deleting the page it left loses nothing. A version the page
     already holds with the same bytes is reused (a thread moved back
     writes nothing); otherwise a plain copy is written, noted `moved`
     (owner ruling 2026-10-06: no hard links; a version referring to
     another page's files was judged too invasive for now). After copies,
     a page that existed gets the version it showed (its current version
     when the move was staged; never "the newest not moved", which can be
     a placeholder) written once more on top, with its own note; without
     copies its current version is left alone.
   - **Agents** (review H1). Watches are per page, so the thread's agents
     would lose it: every live session watching the page it left, the
     session it was sent to and every session with feedback of it not yet
     acknowledged watch the new page too, with their arming, and so does
     every scope watch covering the path it was made at.
   - **The writer.** The store has one writer, and a move may compare and
     copy many megabytes, so (as `write_version_then` does) the files are
     compared and copied into staging directories before the writer is
     taken; the writer is held only to check that each page is at the
     version and each thread on the page with the links staging saw (else
     staging runs again), insert the rows and rename the directories. One
     call stages at most 64 MiB of source snapshots; past that, its other
     threads wait for the client's next request (`remaining`).
   - **Atomic.** The re-filing is one transaction; a failure leaves nothing
     of it, and the pages the request made that hold no thread are deleted
     again.
   - A scope-made watch carried to a page none of the session's scopes
     covers ends when the session removes a scope watch of that origin.
   - Each move is recorded (`thread_moves`: who, from which URL, to which,
     its kind (`move`, `merge`, `unmerge`) and rule). The stream gets the
     new page's `version`s, `thread_moved {artifact_id, thread_id,
     to_artifact_id}` on the page left, then `thread` on the new page.
     Until the shell and the extension handle `thread_moved`, the page left
     also gets `thread_deleted` on its own topics (not `site:`), so they
     drop the thread. Moving a thread to where it is writes nothing.
4. **Merging.** A merge rule `{origin, pattern}` maps every path of the
   origin that `pattern` matches to one canonical live page whose path is
   the pattern (`/users/:id`), marked in page views (`merged`, `pattern`).
   Patterns are `/`-separated literals, `:name` (one non-empty segment) and
   a last `*` (one or more segments), with at least one literal and one
   `:name` or `*` (owner ruling 2026-10-06: `/*`, `/:x`, `/:a/:b` would
   merge a whole site and are refused); no regular expressions; at most 256
   bytes, 16 segments, 64 rules per origin; the daemon's own origin is
   refused. A rule's own pattern always names its canonical page; otherwise
   the most literal segments win, then the most `:name` segments, then the
   oldest rule. Adding a rule re-files, as a move does, every thread of the
   origin whose path the rule now wins, at most 200 a request (each batch
   one transaction; the answer's `remaining` tells the client to repeat;
   a repeat copies nothing twice). From then on every lookup and write for
   a matching URL (`GET /api/live/pages`, `POST /api/live/threads`, `POST
   /api/live/snapshots`) resolves to the canonical page, and a thread made
   there keeps its URL's path (`threads.live_path`), by which scope watches
   still cover it and by which snapshots settle pending addresses: a
   snapshot of `/users/2` links only threads made at `/users/2`.
5. **Un-merging** (owner ruling 2026-10-06). Deleting a rule takes it out
   of force at once and moves each thread on its canonical page made at
   another path than the pattern's (merged, made at a mapped URL, or moved
   by the owner to one) back to the page of that path, or to another rule's
   canonical page when one maps that path, 200 a request; only threads made
   at, or moved onto, the pattern's own path stay. The rule stays
   `deleting` until none is left, and is then removed only if still
   deleting (a re-add meanwhile keeps it in force).
6. **Who.** Reads need an owner credential; moves and rule changes need the
   token or the extension's credential. All of it is under `/api/live/`, so
   hidden from the LAN (L10) and admitted by the extension gateway's
   allowlist (§9.2).

Migration 18 adds `threads.live_path`, `live_rules (id, origin, pattern,
created_at, deleted_at, UNIQUE (origin, pattern))`, `thread_moves (id,
thread_id, from_artifact_id, from_url, to_artifact_id, to_url, moved_by,
kind, rule_id, created_at)` with its index by thread, and an index of
`live_picks` by thread.

### 7.2 Joined sites

Problem: a dev server that moves from `localhost:7702` to `:7703` is two
origins to Clax, so its threads do not list together, an agent watching one
port does not hear the other, and an agent had to be told to look for the
old comments.

Owner decisions, 2026-10-06 (binding):

1. **Clax suggests, the owner confirms.** When Clax is turned on at an
   origin that joined no site, and another origin of the same host family
   (`localhost`, `*.localhost`, `127.0.0.0/8`, `[::1]`, one scheme) has
   live pages of the tab's path, or titled as the tab is, or of a path the
   origin's own pages have, the side panel says "Looks like localhost:7702 —
   same app?" with Join, Not now (a day) and Never (for that pair), the
   answers stored by the daemon. Nothing joins on its own. The panel's site
   tools (Addresses) and the gallery's site menu join an origin to any site
   ("Same app as…") and split an origin off again.
2. **Joined means shared.** (a) Pages are matched by path whatever the
   origin: the site has one key (an origin), its pages and rules are kept
   under it, and a joined origin's pages are merged into the site's of the
   same path through the move machinery (batched, each batch atomic,
   history kept as `join` moves); threads from every origin list together
   and pin on any of them. (b) A watch on any origin of the site, page or
   scope, covers them all, origins joined later included. (c) Merge rules
   apply across the site. (d) The gallery has one entry per site, named
   after its most recently used origin, listing the others.
3. **Links.** Opening a thread goes to its path on the site's most recently
   used origin; the worker probes it with a short request (1.5 s; an origin
   outside its `connect-src`, `http://localhost:*` and `http://127.0.0.1:*`,
   cannot be probed and counts as answering) and tries the others, the most
   recently used first, then says that none answers. A navigation within
   the site keeps Clax on in the tab, now for the new origin (per-tab
   enablement is otherwise unchanged); joining asks Chrome for the site's
   other origins under the click, so the overlay can follow.

1. **Model.** One migration (19) adds
   `live_sites (origin PRIMARY KEY, site, joined_at, last_used_at)`: a row
   per origin of a joined site, the key's own included; an origin without
   a row is a site of its own, keyed by itself. `live_merged_pages
   (artifact_id, origin, path, merged_into, merged_at)` keeps the pages a
   join merged away. `live_site_answers (a, b, answer, until, created_at)`
   keeps Never and Not now per ordered pair.
   `thread_moves.kind` admits `join`. Every query by origin (a page by key,
   the site's pages, its rules, the scope watches covering a page, a pick)
   resolves the origin to its site's key in SQL, so the rest of §7.1 runs
   unchanged on the key.
2. **Joining** `origin` to the site of `with` keeps `with`'s key. In one
   transaction: the joining origins get rows; each of their pages whose
   path the site lacks is re-keyed; their rules become the site's (a
   pattern the site has is dropped); when any origin of the site has a
   scope watch, every page of the site is watched by them. The pages whose
   path the site has stay under their origin, pending (listed with the
   site's, marked `pending`): each request re-files at most 200 of their
   threads onto the site's page of the path (64 MiB of snapshots, as
   moves). A pending page left empty hands its watchers on and is merged
   away, never deleted (owner ruling 2026-10-07: joining deletes no data):
   its row moves to `live_merged_pages`, naming the site's page, so it is
   no longer a key (its origin may make a page of that path again after a
   split) and leaves the listings and the gallery, while its artifact, its
   `/a/<id>` link and every snapshot stay; the shell links it to the page
   it was merged into ("Merged into …"), and a new thread on it is refused
   (409 `merged_away`). Deleting that page releases it in the same
   transaction (owner ruling 2026-10-07): listed and deletable again,
   never stranded hidden. Once none is pending the site's
   rules are applied across it. The client repeats the request while
   `remaining` is above 0; a repeat is idempotent. While a join of either
   site is not finished, another join of them, and any split, is refused
   (409 `joining`): the owner continues it first (the panel says how many
   threads are left, with Continue joining; the gallery with Finish
   joining). A join is refused (409 `unmerging`) while either site has an
   un-merge under way, so no rule or un-merge is lost.
3. **Splitting** removes the origin's row. What the site holds stays with
   it (history stays: threads made on the split origin are not moved back);
   from then on the origin's lookups and new pages are its own. When the
   key is split off, the key moves to the most recently used origin left,
   with the site's pages and rules. Any split is refused (409 `joining`)
   while a join of the site is not finished. A site of one origin left
   loses its row. The pair is answered Never. Scope watches keep what they
   made until the session removes a scope watch: then those no scope of it
   covers go, the split-off origin's included.
4. **Realtime.** A live page's events go to the `site:` topic of every
   origin of its site (the daemon keeps the memberships in memory with the
   live pages, reloaded at each join and split), and a `site` event names
   the site's origins and those that left. The extension's listing and
   the gallery's cards reload on it (the gallery hears it too).
5. **Who.** All of it is under `/api/live/sites/` (hidden from the LAN,
   L10, and in the router's coverage test); reads need an owner
   credential, writes the token or the extension's credential; the
   extension gateway admits each route. Suggestions are only for one host
   family; joins are allowed for any origins the owner picks except the
   daemon's own.
6. **Agents.** `watch` on a URL covers its site; its result names the site
   and its origins. The skill says a thread may come from any of a site's
   addresses.

## 8. Screenshots and snapshots

### 8.1 The screenshot (the clip)

At the pick the overlay hides its outline, pin cursor and area box, waits two
animation frames, and asks the worker to capture. The worker calls
`chrome.tabs.captureVisibleTab(windowId, {format: "png"})`, which needs
`activeTab` (L8). On an `OffscreenCanvas` it draws the anchor's rectangle
(viewport CSS pixels times the capture's scale) as a 3 px red-orange outline
(`#ed5439`, the pin colour), scales to at most 1600 px on the long side, and
encodes PNG; over 5 MiB it halves the scale, up to three times, then gives up
with `clip_too_large`. Without `activeTab` the thread is posted with no clip
and `clip_error: "no_capture_permission"`, which the composer shows ("Click
the Clax button or press ⌥⇧C to comment with a screenshot").

### 8.2 The snapshot

Serialized from the live DOM by the overlay, after the composer has focus.

- Removed: `script`, `noscript`, `template` (unless a declarative shadow
  root), `base`, `meta[http-equiv]`, `link` other than stylesheets, HTML
  comments, the overlay's own host, `input[type=hidden]`.
- Replaced by an empty `div` of the same rendered size with
  `data-clax-placeholder="<tag>"`: `iframe`, `frame`, `object`, `embed`,
  `canvas`, `video`, `audio`, and any element over the size caps below.
- Attributes removed: every `on*`, `srcdoc`, `nonce`, `integrity`, `action`,
  `formaction`, `ping`; `value` on every `input`; any URL attribute (`href`,
  `src`, `srcset`, `poster`, `xlink:href`, `background`) whose scheme is not
  `http`, `https`, `data:image/*` or `blob:` (`blob:` is dropped too, since
  it dies with the page). `textarea` content is emptied. `select` keeps its
  options and the selected one.
- URLs are made absolute against the document's base URL.
- Styles: each `<style>` element and each readable stylesheet `<link>`
  becomes a `<style>` holding its CSSOM rules' text (so rules inserted with
  `insertRule`, as CSS-in-JS libraries do, are kept), with `url(…)` made
  absolute against the sheet's URL; `adoptedStyleSheets` of the document
  become `<style>` elements too. A stylesheet whose rules cannot be read
  (cross-origin without CORS) stays a `<link>` to its absolute URL.
- Open shadow roots become declarative shadow DOM
  (`<template shadowrootmode="open">`), serialized by the same rules; closed
  ones are omitted.
- Caps: 100,000 elements, 8 MiB of HTML, 1.5 s of serialization. Past any
  cap the snapshot is replaced by a minimal page holding the title and
  "Snapshot unavailable: the page is too large".

The result is a full document (`<!doctype html><html …><head>…`) whose
`<head>` starts with `<meta charset="utf-8">`. No `<base>` is added: every
URL is already absolute.

### 8.3 Storage

`POST /api/live/threads` stores the snapshot as `index.html` of a new version
when it differs from the current version's bytes (L4), with the page's title
(cut to 200 characters, control characters dropped) as the version's title
and the note `snapshot`. Versions of live pages carry no other files.

### 8.4 Serving a snapshot

Every HTML response of a live page's version (`/c/<aid>/v/<n>/…` and
`<aid>.localhost`) carries, besides the policy it carries today, a second
`Content-Security-Policy`:

```
script-src http://<Host>/_clax/; object-src 'none'; base-uri 'none';
form-action 'none'; frame-src 'none'; connect-src 'none'; worker-src 'none'
```

where `<Host>` is the request's `Host`. Both policies apply. Only the bridge
and its parts run; inline scripts, event handlers and every other script
source are blocked, whatever the snapshot holds. Styles, images and fonts may
load from anywhere (the page's dev server, while it runs).

## 9. Protocols

### 9.1 Native messaging

Chrome starts the host per message (`chrome.runtime.sendNativeMessage`),
with the caller's origin as the first argument, and frames each message as a
32-bit native-endian length and that many bytes of UTF-8 JSON.

The host:

1. Refuses to run unless its first argument is exactly
   `chrome-extension://<the ID in effect>/` (exit 1 with an `error` reply,
   `wrong_origin`).
2. Reads one message of at most 64 KiB (`bad_request` otherwise).
3. Answers it, writes one reply of at most 64 KiB, and exits 0. Nothing but
   framed replies is ever written to stdout; logs go to
   `~/.clax/logs/native-host.log`.

Messages:

```json
→ {"type": "pair", "v": 1, "extension_version": "0.9.0"}
← {"type": "paired", "v": 1, "daemon": "http://localhost:7480",
   "credential": "cxe_…", "clax_version": "0.9.0",
   "viewer": {"public_id": "u_…", "display_name": "Alex"}}
← {"type": "error", "v": 1, "code": "daemon_unavailable" | "bad_request" | "wrong_origin" | "unsupported_version", "message": "…"}
```

`pair` ensures a daemon (the discovery and auto-start of §7, which may take
up to 5 s), then mints a credential with the token (`POST
/api/extension/credentials`). `viewer` is the owner viewer's public view
(L6), as the mint returns it. A message whose `v` is not 1, or of another
type, is `unsupported_version` or `bad_request`. Chrome's own check of
`allowed_origins` comes first; the host's check of its argument covers a
manifest edited by hand.

### 9.2 Extension ↔ daemon

Every request from the worker carries `Authorization: Clax-Extension
<credential>`, with `credentials: "omit"`, and `Origin:
chrome-extension://<ID>` (added by Chrome), except a GET while the
extension holds a host permission for the daemon's origin (`<all_urls>`, or
"On all sites" in chrome://extensions): Chrome sends that one without
`Origin`, marked `Sec-Fetch-Site: none`, which no web page can send. The
gateway counts a GET with the credential, no `Origin` and
`Sec-Fetch-Site: none` as the extension's; any other request with the
credential and no `Origin` (another method, or another `Sec-Fetch-Site`)
is 403 `forbidden_origin`. (Provisional, pending the owner's confirmation:
found by the browser test, 2026-10-05.) New routes (viewer routes: no token):

- `GET /api/live/pages?url=<page URL>` → `{page: {artifact_id, origin,
  path, page_url, title, current_version, url} | null, route}`. Never
  creates.
- `POST /api/live/threads` (multipart, ≤ 24 MiB): `url`, `title`, `anchor`
  (JSON, without `route`), `body`, `pending` (a JSON array of the thread IDs
  the extension saw pending when it serialized the page; a new version links
  those still pending, in its transaction, per the L11 ruling; any other
  field is 400 `invalid_args`), optional `pick_id` (the pick ID, §9.4: 32
  lowercase hex digits, else 400 `invalid_args`; the extension sends it on
  every attempt), optional `clip` (PNG), `snapshot` (HTML, ≤ 8 MiB). Finds
  or creates the live page (with its scope-watch materialization), stores
  the snapshot (§8.3), creates the thread on the resulting version with
  `route` set, handles `@agent`, and answers
  `201 {thread, page, version, clip_error?}`. With `pick_id`, a repeat
  within an hour whose pick already made a thread on the page, while that
  thread still exists, writes and sends nothing and answers
  `200 {thread, page, version}` with that thread as it is now (`version` is
  the one it was made on); two such requests at once make one thread. So
  the worker's retry after a dropped connection (§11) makes no second thread
  and no second `@agent` send. Deleting the thread frees its pick ID.
- `POST /api/live/snapshots` (multipart): `url`, `title`, `pending` (a
  JSON array of the thread IDs the extension saw pending when it serialized
  the page), `snapshot`; any other field is 400 `invalid_args`. Refused with
  409 `nothing_pending`, writing nothing, unless some thread of `pending`
  still has a pending address on the page; otherwise stores the snapshot as
  a new version (even when identical, so the link has a version after the
  address), links those threads' pending addresses (others stay pending),
  and answers `{page, version, linked: [thread IDs]}`. The check, the
  version and the links are one transaction. (Ruling 2026-10-05: the
  snapshot names the pending threads it covers.)

Site-wide threads (§7.1, owner decision 2026-10-06): `GET
/api/live/site?origin=`, `POST /api/live/threads/<tid>/move {page_url}`,
`GET /api/live/rules?origin=`, `POST /api/live/rules {origin, pattern}` and
`DELETE /api/live/rules/<id>`; the gateway admits each.

The gateway also admits, for live-page artifacts only: `GET
/api/artifacts/<aid>`, `GET …/threads`, `GET …/threads/<tid>`, `GET
…/threads/<tid>/clip`, `POST …/threads/<tid>/comments`, `…/send`,
`…/threads:send`, `…/resolve`, `…/reopen`, `DELETE …/threads/<tid>`, `GET
…/working`, `GET …/presence`; and `GET`/`PUT /api/viewers/me`, `PUT
/api/viewers/me/looked`, `PUT /api/viewers/me/presence`, `GET
/api/stream`, `POST /api/stream/<id>`. Anything else from the extension's
origin is 403 `forbidden`.

Token routes:

- `POST /api/extension/credentials` `{extension_id}` → `{credential,
  viewer, expires_in_s}`, where `viewer` is the owner viewer (`{public_id,
  display_name}`); 400 `unknown_extension` for any ID but the ID in effect (L15).
- `GET /api/extension` → `{extension_id, live_credentials, last_used_at,
  viewer}` (`viewer` as above).
- `DELETE /api/extension/credentials` → revokes all; `{revoked: n}`.

### 9.3 Agent-facing changes

`watch` with a page URL: `PUT /api/sessions/<sid>/live-watches` `{url,
replies_armed}` → `{live_watch: {origin, path, scope, replies_armed}, page,
covered: [artifact IDs]}`; `DELETE /api/sessions/<sid>/live-watches?url=`.
The tool's result is `{artifact_id, url, page_url, scope, watching,
replies_armed}`.

Payload of a comment on a live page:

```
[clax] Comment sent to you on "Settings" (live page http://localhost:5173/settings?tab=billing; Clax view http://localhost:7480/a/7q3k9mzx2b4t), thread 01J9...
Anchored on: ?tab=billing › main > form > button  «Save»  (snapshot v3)
Clip: /Users/alex/.clax/artifacts/7q3k9mzx2b4t/clips/01J9....png
Snapshot: /Users/alex/.clax/artifacts/7q3k9mzx2b4t/versions/3/index.html
Alex: "The save button overflows at phone width."
Reply with comments_reply (addressed: true once the page shows the fix), then comments_resolve when done.
```

`comments_read` thread entries gain `page_url` and `snapshot_path` for live
pages, and `addressed_pending` (boolean) on every thread.

`comments_reply` with `addressed: true` on a sent thread of a live page
records a pending address (`explicit`) and answers `{…, addressed:
"pending"}`; on another artifact it is 400 `invalid_args` ("addressed is for
live pages; publish with addresses instead"). An agent resolve on a live
page records a pending address (`resolve`) when the thread has no link yet,
instead of linking the current version.

### 9.4 Inside the extension

All messages are JSON objects with a `t` field, validated by one function
per receiver (`isFromOverlay`, `isFromWorker`, `isFromComposer`,
`isFromPanel`) that checks every field's type and bounds; anything else is
dropped and counted.

- The worker accepts a runtime message only when `sender.id ===
  chrome.runtime.id`; an overlay message only from a tab's top frame
  (`sender.frameId === 0`) of a tab Clax is on in, whose origin is the one
  Clax is on for there (any other overlay message is answered `{off:
  true}`, and the overlay stops); a
  composer port only when `sender.url` is the extension's `composer.html`
  (under its ID or its dynamic one), the sender is a frame of the pick's tab
  (`sender.tab.id`, `sender.frameId > 0`), and the port names the tab's
  current pick: a pick ID (128 random bits) the overlay drew and the worker
  took for that tab with its `capture`, within 5 s of the worker's
  `open-composer` for it, one port per pick; a panel port only from `sidepanel.html`.
- The overlay accepts messages only from the worker (`sender.id ===
  chrome.runtime.id` and no `sender.tab`).
- Overlay → worker: `route {url}` (a same-document navigation, or the
  overlay's start),
  `capture {pickId, anchor, rect, dpr}`, `pick {pickId, snapshot |
  snapshotError, url, title}` (`snapshotError`: `too_large`, with the
  serializer's placeholder, or `failed`, when the serializer threw, with no
  snapshot), `resolved {results}`, `quiet {url, title, snapshot, pending}`
  (`pending`: the thread IDs its state showed waiting for a snapshot when it
  serialized the page, L11), `cancel {pickId}`, `comment-mode {on}` (the
  person left comment mode with Escape), `pin {threadId}` (a pin was
  clicked), `removed` (the page removed the overlay's host a second time),
  `ping` (every 20 s, §9.5). The `url` of `route` and `pick` is
  null when the page's address is over 4096 characters (§7): the worker
  then looks nothing up and the panel says "This page's address is too long
  for Clax."; a pick is refused at once with that notice, and a post whose
  `pick` came with no URL fails in the composer with it. `quiet` is not sent
  for such a page. Route reports count only the browser's own (`isTrusted`) navigation events, read the URL
  from `location`, skip an unchanged URL and send a burst's last URL once,
  at most one every 250 ms.
- Worker → overlay: `state {page, route, threads, commentMode, pending}`,
  where each thread is only `{id, status, anchor, addressed_pending}`, and
  a thread of another page of the site also `from` (the path it was made
  at; owner decision 2026-10-06) (`addressed_pending` a boolean; no comment text, replies, names or
  feedback state reach a content script, L7), sent only when one of these
  fields changed since the overlay was last told and in full to a new
  overlay; `captured {pickId, ok, error?}` (the answer to `capture`),
  `open-composer {pickId, rect}`, `composer-ready {pickId}`,
  `close-composer {pickId, posted, reason?}` (`reason: "timeout"` when the
  composer page never connected: the overlay says so in a short notice),
  `pick-lost {pickId}` (the worker no longer holds the pick whose composer
  is shown: comment mode comes back, and the composer stays, with its text,
  until the person closes it or picks again), `scroll-to {threadId}`,
  `focus {threadId | null}`, `resend` (the worker has no results for the
  threads it shows: the overlay sends its `resolved` again), `off` (Clax
  turned off in the tab: the overlay stops, its pins and comment mode with
  it). Messages go to the document whose overlay last wrote (`documentId`), so a
  newer document in the tab never hears the old one's state.
- The composer frame is shown only after the worker confirms its page
  (ruling 2026-10-05): the overlay inserts the frame hidden (so it cannot
  take focus); the composer page connects its port only after its own
  `load`; once the worker takes that port it sends `composer-ready`, and only
  then does the overlay show and focus the frame and serialize the page,
  while the person types. A second `load` of the frame (a navigation the
  page made) closes it and cancels the pick; so does a frame whose page has
  not connected within 10 s, and the worker cancels a pick whose composer
  has not connected in that 5 s wait. A document the page puts in the frame is
  never shown or focused.
- Composer ↔ worker (port named `composer:<pickId>`): `ready` → `draft
  {anchor, clipUrl | null, clipError | null, capturing}` (`clipError` a
  code the composer words); `post {body}` → `posted {threadId}` | `failed
  {message}` (a failure to pair worded as the side panel words it); `cancel`.
  Once its port is gone unasked (the worker was stopped), the composer page
  sends one-off runtime messages, which the worker takes only from a
  `composer.html` frame of the tab: `lost {pickId}` (the worker answers the
  overlay with `pick-lost` for a pick it does not hold) and, when the person
  then closes it, `dismiss {pickId}` (the worker answers `close-composer`).
  Cancel never throws on a closed port.
- Panel ↔ worker (port `panel:<windowId>`): `watch-tab` → `tab {state}`, where `state` is `{tabId, url, page, route,
  threads, resolved, versions, working, participants, viewer, commentMode,
  enabled, declined, selected, error, presence?}` (`enabled`: Clax is on in
  the tab; `declined`: the person refused the site's permission when
  turning it on, so a reload will turn it off; `resolved`: the overlay's anchor results by thread ID;
  `presence`: who is on the page, §9.5), and pushes on change, with
  `stream-status {up}` on `watch-tab` and whenever the stream goes up or
  down; `send`, `send-batch`, `reply`, `resolve`, `reopen`, `looked`,
  `set-name`, `select`,
  `comment-mode`, `navigate {route, artifactId}`, `turn-off {tabId}`,
  `retry` → `failed {code, message}` on failure (a failure a new pairing can
  fix is also kept as the tab's `error`, so Retry pairs again for it), and success shows as the
  next `tab` push (ruling 2026-10-05: no `ok`). `navigate` names the page
  the panel showed; the worker refuses it (`page_changed`) when the tab
  shows another. `turn-off` turns Clax off in the panel's tab (shown as
  "Turn off in this tab" whenever Clax is on there); it names that tab,
  and the worker refuses it (`page_changed`) for another. A panel page is
  pinned to its tab (`sidepanel.html?tab=<tabId>`) and watches only it. `visible {on}` says whether
  the panel's document is visible: the worker reports presence only while
  it is (§9.5). `ping` every 20 s.

### 9.5 Realtime

The worker runs the shell's stream `Hub` (`stream-hub.ts`) with an
environment whose requests carry the credential, against `/api/stream`, as
one client per tab and one per panel. Topics: `artifact:<aid>` and
`working:<aid>` for each live page an overlay or the panel shows, and
`presence:<aid>` while the panel shows it, and `site:<origin>` for the
panel's "Elsewhere on this site" (§7.1). A stream opened through the
gateway is live-only: subscribing to `gallery`, `docs:*` or a topic of an
artifact that is not a live page is refused 403 `forbidden`. The worker
stays alive while a panel or overlay port is open; each sends a ping every
20 s. When Chrome stops the worker anyway, the next message starts it, and
the hub resumes the stream with `Last-Event-ID` within the daemon's 60 s hold
or refetches.

Presence: while the panel shows a live page and is visible, the worker
reports the owner `here` on it every 30 s. It never reports `away`: presence
is keyed by viewer, so an `away` from the panel would also mark the owner
away in a shell tab showing the same page. When the panel hides, the report
lapses (the main spec's §10, "Presence") unless another of the owner's clients
keeps it.

## 10. Security model

What is protected, from whom:

1. **The daemon token** never leaves the daemon, the CLI, the native host and
   the shell on localhost. The extension never sees it: the native host uses
   it to mint a credential and returns only the credential.
2. **The extension credential** lives only in the worker's
   `chrome.storage.session` (memory only; the default access level keeps it
   from content scripts). It is never put in a URL, a DOM, a message to the
   overlay or composer, or a log. It grants exactly the gateway's allowlist,
   on live pages only, as the owner identity (L6). It cannot publish,
   delete artifacts, read sessions, read the token, use `db`, or see any
   non-live artifact. A stolen credential therefore acts as the owner on
   live pages (comments, sends, resolves, the owner's name and marks, and
   since §7.1 moving threads between a site's pages and its merge rules)
   until revoked; that is the cost of one identity, bounded by the
   allowlist.
3. **The page cannot drive Clax.** No `externally_connectable` and no external-message listener; the
   isolated world's messages are validated; CommentMode acts only on trusted
   events (`isTrusted`), so page scripts cannot pick; posting a comment takes
   a click or key inside the composer frame, which the page cannot script;
   Send, Resolve and every other write happen only in the side panel or the
   composer.
4. **The page cannot read Clax.** Pins and outlines are in a closed shadow
   root and carry no text; the composer and every thread's text are in
   extension frames. The page can see that a `<clax-overlay>` element exists
   (so it can tell Clax is on) and its size, and can remove it; removal is
   detected by a MutationObserver and the host is re-added once, then the
   overlay gives up and says so in the panel. While the overlay draws, a page
   script can also observe (checked in Chromium by the browser test where
   marked):
   - **Pin positions.** Pins take the pointer, so hit testing
     (`elementFromPoint`, or where its own pointer events stop arriving)
     finds the host at each pin; it learns where pins are, never their
     threads or text.
   - **The cursor.** Comment mode draws its outline in a closed root, but
     the page hears its own pointer events as ever, so it knows where the
     person points; it cannot tell which element comment mode outlines.
   - **The capture moment** (checked). The host's `style` attribute gains
     `visibility: hidden` for the two frames of the screenshot, then loses
     it; a MutationObserver on the host sees both.
   - **Focus during compose** (checked). The composer frame takes focus:
     the page sees its window blur and `document.activeElement` become the
     host; the keys typed reach the composer, never the page. The frame is
     not in the page's `window.frames` (checked), and a page cannot frame
     `composer.html` itself (`use_dynamic_url`; checked: Chrome refuses it).
   - **A modal dialog** (checked). While the page has a modal `<dialog>`
     open, the rest of the page is inert, the overlay's host with it: the
     dialog's backdrop takes the pointer over pins and the composer. A pick
     inside the dialog still opens the composer, which has focus, so typing
     and the composer's submit shortcut work; its buttons cannot be
     clicked until the dialog closes.
5. **Snapshots** are page content: hostile and possibly personal. Sanitized in
   the extension (§8.2: no scripts, no handlers, no form values, no hidden
   inputs); served with the script-blocking policy (§8.4); visible only on
   this machine (L10).
6. **The daemon** accepts the extension's origin only with a live credential;
   a bearer token from the extension's origin is refused 403 (no route mixes
   the two). The gateway strips the request's `Origin`, `Cookie`,
   `Sec-Fetch-Site` and `Authorization` before handing it to the existing
   handler as the owner identity through the owner identity's hook (§2.1),
   so the existing `SameOrigin` and viewer rules apply unchanged, and the
   response carries
   `Access-Control-Allow-Origin: chrome-extension://<ID>` and `Vary: Origin`.
   Preflights are answered only for that origin and the allowlisted paths.
   The `/api` host rule (DNS rebinding) is unchanged: the worker uses
   `localhost` or `127.0.0.1`.
7. **The native host** is reachable only from the extension's origin
   (`allowed_origins`, plus its own argument check). Any local process can run
   `clax native-host` and get a credential, but every local process can
   already read `daemon.json`; nothing is gained.
8. **Untrusted text**: comment bodies, page titles, page URLs, quotes and
   snapshots are untrusted. Svelte renders them as text; payloads quote them
   as JSON strings (§10 "Feedback payload"); the URL in a payload is the
   normalized one, written with `one_line`.
9. **Permissions**: nothing runs in a tab until the person turns Clax on
   in that tab; the persistent per-origin grant is Chrome's own prompt, and
   holding it turns Clax on nowhere by itself: it only lets the worker
   inject the overlay again after a reload of a tab Clax is on.

## 11. Failure modes

| Failure | What happens |
|---|---|
| Native host not registered (plugin-only install, `clax init` never run) | `sendNativeMessage` fails with "Specified native messaging host not found"; the panel says "Run `clax init` (or /clax:extension in Claude Code), then reload" with a copy button. |
| Daemon cannot start | The host answers `daemon_unavailable` with the log path; the panel shows it and a Retry button. |
| Daemon restarted on another port, or credential revoked | A request fails with a network error or 401 `unknown_credential`; the worker re-pairs once (at most every 10 s) and retries the request once; after a network error a comment, a send or a batch send is not sent again (the daemon may have done it): it fails with `daemon_unreachable`, and the panel's Retry pairs again. A new thread is sent again: every attempt names its pick (`pick_id` in `POST /api/live/threads`), of which the daemon makes one thread. Within 10 s of the last pairing it fails with `daemon_unreachable` (or the 401); the panel's Retry pairs again at once. |
| Loaders an earlier, per-origin build registered | Unregistered at each worker start, with that build's list of origins: they would bring Clax to every tab of their origin. |
| Worker stopped by Chrome mid-stream | Resumed by the next event; stream resumes with `Last-Event-ID` or refetches (§9.5). The tabs Clax is on come back from session storage; their side panels are enabled again. An icon click the worker hears before it read them back opens the panel, then turns Clax off if the tab was on. |
| Extension files older than the daemon | On pairing, `clax_version` differs from the manifest's version: the worker calls `chrome.runtime.reload()` once for that version (remembered in `storage.local`). |
| Extension reloaded (L13) or updated while a tab has an overlay | The old overlay is orphaned. Presence means a live overlay of this load of the extension: the overlay marks its isolated world with a check of its own context and of the boot nonce the worker keeps in session storage (new at each load of the extension). An orphan is not present, so the next gesture injects again, and the new overlay stops the orphan when they share the world. The reload itself waits until no pick is open and no request is in flight. |
| Worker stopped by Chrome while a composer is shown | The composer says "Clax stopped listening to this comment. Copy your comment, then pick again." and tells the worker (`lost`); the overlay gives comment mode back with the composer still shown; Cancel closes it (`dismiss`), and a new pick replaces it. |
| Page address over 4096 characters | Nothing is looked up; the panel says "This page's address is too long for Clax."; a pick shows that notice instead of a composer, and a post fails with it. |
| Post pressed before the page's snapshot came | The post fails after 30 s with "The page did not send its snapshot. Post again." |
| The connection drops after the daemon stored a comment (the native host replacing the daemon mid-request) | The worker retries with the same `pick_id`; the daemon answers 200 with the thread it made, so there is one thread and one send. |
| No `activeTab` at a pick | Posted without a clip (§8.1), with the reason in the composer. |
| Snapshot over the caps | A minimal placeholder snapshot that says why; anchors resolve on the live page and detach in the shell. |
| The serializer fails on a page | The pick is sent with `snapshotError: "failed"` and no snapshot; the thread is posted with a placeholder snapshot that says the page could not be read. |
| The composer page never connects | The worker cancels the pick 5 s after `open-composer`; the overlay closes the hidden frame and says "The comment box did not open. Pick again to comment." |
| Page removes or restyles the overlay host | Re-added once; then the panel says the page removed Clax's overlay. Pins use `all: initial` and the top layer (`popover`). |
| Tab Clax is on reloads, or navigates within its origin | Clax stays on: once the new document has loaded the worker injects the overlay into that tab again; comment mode is off; the worker treats `activeTab` as gone until the next icon click or command. It probes the tab for the overlay at `loading` and again at `complete` (an in-page navigation keeps its overlay), and a probe answered after a new document or an injection it saw meanwhile resets nothing. |
| Tab Clax is on navigates to another origin | Clax turns off in the tab: its panel is disabled, its state dropped. The worker sees the new URL where it may read it; any navigation update whose URL it may not read (an origin it holds no permission for, or a reload once the person declined the permission, if Chrome then withdraws `activeTab`: whether Chrome keeps `activeTab` across a same-origin reload is to be checked by hand, docs/verification.md) turns it off too, at `loading`. Back on the first origin it stays off until the person turns it on again; a page restored from the back/forward cache asks the worker at once, is refused, and its overlay stops. |
| Another tab of the same site, or a new tab | Nothing: no overlay, no panel, no state, whatever permission Clax holds; a message from an overlay there is dropped. |
| Tab closed, or replaced under a new ID (prerender, discard) | Its state, pick and stream topics go; a replacing tab starts off. A closed tab's ID is kept for 5 minutes, so an answer still in flight writes no record for it. |
| On tab moved to another window | Its panel goes with it and still acts on it (`?tab=`); presence is reported for its new window. |
| SPA route change | The overlay reports the new URL; the worker looks it up (`GET /api/live/pages`) and switches artifact or route; pins follow. |
| Hot reload replaces the DOM | Mutations re-resolve anchors within one animation frame after a 150 ms quiet period; a thread whose anchor is gone goes to Detached. |
| Agent addressed a thread while the page is closed | The address stays pending; the thread shows "waiting for a snapshot" until the page is next open in Chrome with the extension. |
| Two Chrome profiles | Each loads the extension (same ID) and pairs separately, with its own credentials; both act as the owner identity (L6), as the shell in any browser on this machine does. |
| Port of a dev server reused by another project | Same live pages (§7). The person deletes the live page to start over. |
| LAN-bound daemon | Extension works (it talks to `localhost`); LAN viewers never see live pages (L10). |

## 12. Time to usable

- A tab Clax is not on in loads nothing of Clax. In a tab it is on in, the
  worker injects `overlay.js` with `chrome.scripting.executeScript` once
  each document has loaded (no web-accessible script, no fetch by the
  page).
- The icon click injects the overlay at once, before pairing finishes; comment
  mode is on as soon as the overlay runs. Pairing and the page lookup run in
  parallel.
- Budgets in `web/perf/bundle-budget.json`: `overlay.js`
  32 KiB, `sw.js` 24 KiB, `sidepanel.js` 64 KiB, `composer.js` 28 KiB (gzip).
- The e2e test reports (never judges) icon-click → comment mode on, and pick →
  composer focused.

## 13. Testing

- **Rust**: URL normalization cases (`crates/clax-core/src/live-url-cases.json`);
  migrations 16–17 from the previous schema; live-page store (create,
  placeholder, snapshot dedupe, delete frees the key, pending address
  linking); scope-watch materialization and removal; the routes; the
  gateway (each allowlisted route works, everything else refused, wrong
  origin, revoked credential, bearer from the extension origin, CORS
  preflight, and the extension acting as the owner identity: its
  `/api/viewers/me` is the owner viewer, its name and looked-at marks are
  the owner's); L10 hiding; the snapshot policy header; native host framing and
  origin check against the real binary; `clax extension install/uninstall`
  against `CLAX_NATIVE_HOST_DIRS` scratch directories; MCP `watch` with a
  page URL and the artifact-reference rule.
- **Vitest (jsdom)**: the snapshot serializer (every removal rule, URL
  rewriting, CSSOM text, shadow roots, caps); message validators; the
  worker's pairing and re-pair logic and per-tab enablement (another tab of
  the origin and a new tab get nothing; reloads and navigations within the
  origin inject again; another origin, the icon again and closing turn it
  off; a worker restart restores it; the panel enabled only for on tabs)
  against a fake `chrome`; route matching and the re-resolution scheduler.
- **Playwright** (`web/e2e/chrome-overlay.spec.ts`): Chromium with the
  unpacked test build loaded from `<home>/extension` (`--load-extension`,
  persistent context), the native host registered in the profile's
  `NativeMessagingHosts` by `clax extension install` (with
  `CLAX_NATIVE_HOST_DIRS`), a real daemon, and a real Vite dev server serving
  a fixture app: Chromium's ID equals the daemon's and the CLI's, enable,
  pick, comment, screenshot and snapshot stored, Chrome's own side panel
  (opened by `chrome.sidePanel.open` under a real click, driven over CDP)
  shows it, send, an agent session watching the URL receives it, HMR moves
  and then removes the element (pin follows, then Detached), `addressed:
  true` reply links on the auto snapshot, the shell shows the live page in
  the gallery and the snapshot with its pin, and the shell's viewer is the
  same owner viewer the extension paired as. Also: Retry after the daemon
  restarts on another port; a second tab of the dev server showing no
  overlay and no panel while the first keeps working, a reload keeping Clax
  on, and a navigation to another origin turning it off; and, with only the
  dev server's origin held, the panel's "Turn off in this tab", a
  navigation to an origin the extension cannot read turning it off, a route change keeping
  comment mode, the capture moment, focus and `window.frames`, a post with
  no screenshot, a composer frame loaded again, and a modal dialog.
- The test build differs from the release build only in
  `host_permissions: ["<all_urls>"]` (so capture and injection need no
  gesture Playwright cannot give) and a test hook on the worker that stands
  in for the toolbar click. What only a person can check (the permission
  prompt, the toolbar icon, the side panel opened by the icon, Brave and Edge
  host registration) is listed in `docs/verification.md` as manual.

## 14. Contract and docs

`docs/contract.md` gains a "Live pages" section (key and route, snapshots,
the payload lines, `watch` with a URL, `addressed`), the new routes, the
extension's security model under "Security model", and `clax extension` and
`clax native-host` under "Installation". The main spec gains row D19 in §2
pointing here. The four skills gain a "Live pages" section (§15).

## 15. Skill text

Added to each plugin's `skills/clax/SKILL.md` (tool names per harness):

> **Live pages.** When you run a web app's dev server (or any page the person
> will look at in Chrome), call `watch` with its URL, for example
> `watch({url_or_id: "http://localhost:5173/"})`, right after it starts. That
> watches every page under that URL: comments the person makes there with the
> Clax extension reach you like comments on artifacts, with the page URL, a
> screenshot (`Clip:`) and a snapshot of the page's HTML (`Snapshot:`). After
> your change shows in the page (hot reload or restart), reply with
> `comments_reply` and `addressed: true`; the next snapshot of the page is
> recorded as addressing the thread. You cannot `publish` a live page. If the
> person has not set up the extension, set it up yourself: `clax` is often not
> on PATH, so run this plugin's wrapper, `scripts/ensure-clax.sh exec extension
> install --json` from the plugin's directory (two levels above this skill's),
> or in Claude Code ask them to run `/clax:extension`. Then relay its
> `load_unpacked` step: load `~/.clax/extension` once in Chrome.

## 16. Known limitations

- **An older Clax binary on a migrated database.** Clax binaries released
  before live pages do not refuse a database whose schema is newer than they
  know. If one starts the daemon (an older plugin pin, or an old `clax` on
  `PATH` reached through `clax mcp` or a hook) while no daemon runs, it
  serves the live-pages schema under its own rules: a LAN-bound daemon would
  list live pages to the LAN and serve their snapshots without the snapshot
  policy, until a newer client replaces it on its next connect. Binaries
  with live pages refuse such a database (docs/contract.md, "Known
  limitations"). Mitigation: ship with the plugin pins at or above the
  release that adds live pages, and after a downgrade run `clax stop`.
- **A tunnel to loopback defeats L10.** `sees_live_pages` trusts a loopback
  peer; `tailscale serve`, ngrok or a reverse proxy to the daemon's loopback
  address makes every remote caller look local. This is the owner's
  configuration; Clax cannot tell such a caller apart.
- **The daemon's LAN address is not its own origin (owner design call).**
  §7's `own_origin` refusal knows the local names, loopback and unspecified
  addresses, and the hosts the daemon gives out as its base URLs. Under a
  `0.0.0.0` bind, the machine's LAN address (for example
  `http://192.168.1.5:<port>`) is none of these, so a Clax page opened at
  that address can be commented on as a live page instead of being refused.
  Whether the daemon should enumerate its interface addresses for this
  check is left to the owner.

## 17. Out of scope

- Firefox (MV3 with `browser.*`, no `sidePanel`; a `sidebar_action` port
  later), Safari, Windows (registry-based host registration).
- Snap and Flatpak Chromium on Linux (confined; the host path is not
  reachable). The Claude Code plugin's README says so.
- The Web Store listing itself (the signing scripts prepare it; the upload
  is the owner's manual step, §6.7).
- Commenting on snapshots in the shell (L9).
- Pages inside iframes (the overlay runs in the top frame only), `file://`,
  `chrome://` and the Web Store.
- Headless screenshots of a page nobody has open.

## 18. Open questions

For the owner:

- **Origin-less GETs (§9.2).** The gateway counts a GET that carries the
  credential, no `Origin` and `Sec-Fetch-Site: none` as the extension's,
  because Chrome sends the worker's GETs that way once the extension holds
  a host permission for the daemon's origin. Writes without `Origin` stay
  refused. This ruling is provisional until the owner confirms it.

Keys and signing (L15, §6.7), the composer and how comment mode starts were
decided by the owner (L7, L8), as was the owner identity (L6).
