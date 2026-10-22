# Each artifact on its own origin: `<id>.clax.run`

Status: design, for planning. Date: 2026-10-21.

## Intent

Clax is becoming an application platform: people will host whole sites and
apps as artifacts, with storage, a database and the rest of the page
runtime. Every artifact is therefore untrusted code, written by someone
other than the person viewing it and other than the people hosting it.

Today a hosted Clax (clax.dev) serves every artifact's content from one
shared content origin (`CONTENT_ORIGIN`, e.g. `content.clax.dev`), at
`/c/<aid>/v/<n>/k/<key>/…`, inside frames sandboxed to an opaque origin. That
has three problems:

1. **Artifacts collide.** Every artifact shares one origin (or the single
   opaque `null` origin): one CORS identity, and no storage, cookies or
   service workers of its own. A site that needs them cannot work, and once
   it could, one artifact could read or poison another's.
2. **Content is world-readable.** The key in the path is the only
   credential (`crates/clax-http/src/hosted.rs` `content_key`,
   `content_path`). Whoever has a keyed URL reads that version forever, after
   losing the right to, and whatever the artifact's sharing (open record
   `ad4d8d8c` on `hosted.rs:539:541`, with the same gap for shares, assets'
   `/_blob/<id>?k=` and clips' `/_clip/…?k=`).
3. **App data shares the platform's database.** The `db` capability's
   documents (`docs`, `leases`, `doc_authors`, `artifacts.doc_seq`;
   `crates/clax-store/src/docs.rs`) live in the instance database beside
   people, sessions, rights and threads. A bug in a query or a rights check
   on an app's path is a bug next to the platform's own rows, and one noisy
   app loads the instance everyone shares.

This design gives each artifact its own origin on a separate registrable
domain, `https://<id>.clax.run/`, serving the site from the host's root, with
its own backend and its own data store, reachable only by those the artifact's
access allows. The local daemon gets the same model on `<id>.localhost`.

## Goals

- Each artifact is one origin, `https://<id>.clax.run` (locally
  `http://<id>.localhost:<port>`), on a registrable domain that shares
  nothing with clax.dev: cookies, storage, service workers and CORS identity
  are per artifact.
- The site is served at the host's root, always its **current** version, and
  opens top-level in a tab like any website. clax.dev's artifact page frames
  the same origin for review and comments.
- A private artifact's content (pages, files, assets) is served only to
  someone who may read the artifact now. Losing the right ends access. No
  URL is a credential.
- A new, explicit **open to the world** state per artifact. Only it makes
  the site reachable by anyone, anonymously, without passing through
  clax.dev. It opens the site alone: the artifact's clax.dev page (threads,
  versions, history) keeps its own permissions.
- The page runtime (db, assets, room, …) works on the artifact's own origin,
  framed or top-level, served by that host and able to reach only that
  artifact.
- Each artifact's app data lives in a store of its own, never in the
  instance database.
- The local daemon, the daemon in hosted mode and the hosted Worker
  implement the same model; tests run it on each.

## Non-goals (designed for, not built)

- Vanity names (`my-site.clax.run`) and customer domains by CNAME (Cloudflare
  for SaaS custom hostnames). The host-to-artifact lookup is built so each is
  a new mapping, not a rewrite.
- A platform auth API for apps ("sign in with Clax" inside an app). World
  visitors are anonymous; an app does what it will with its own data.
- Pinning a chosen version as the live site. The root is always current.
- Letting other sites frame an artifact.

## Anti-goals

Any of these makes the change a failure even if it works:

- Any path from an artifact's origin to clax.dev's session, cookies, or API
  with a person's authority. clax.dev needs no CORS for artifacts.
- Any route on an artifact's host that reaches another artifact's content
  or data, or the instance's own tables.
- A credential in a URL that outlives the right it stood for.
- A private artifact readable by someone without the right, through any
  path or host, old ones included.
- The hosted and local models drifting apart.

## Design

### Hosts and the host-to-artifact lookup

- A hosted instance has a **site domain** (new config, replacing
  `CONTENT_ORIGIN`): `clax.run`. A request whose `Host` is
  `<id>.<site domain>` is a **site request** for artifact `<id>`. Locally,
  the site domain is `localhost` and the port is the daemon's.
- `crates/clax-http/src/host.rs` (`artifact_host`, `rewrite_artifact_host`,
  `OnArtifactOrigin`) already does this for `<id>.localhost`. It becomes one
  lookup, `Host → SiteTarget { artifact }`, configured with the site domain,
  which later also consults a table of vanity names and custom hostnames.
  Nothing else in the server parses hosts.
- The lookup runs first, before every route, in the server and in the
  Worker (`crates/clax-worker/src/lib.rs` dispatches `/git/`, `/_diag/`,
  `/_dev/` and the test routes on the path before it looks at the host;
  on a site host those must answer 404). A site host answers only the site
  routes below; the public host serves no content.
- `CONTENT_ORIGIN`, the `/c/…/k/<key>/` scheme, `content_key`, the keyed
  `/_blob/<id>?k=` and `stamp_content_keys` are removed. Old keyed URLs stop
  answering (404): closing them is the point. Shell links (`/a/<id>`) keep
  working.

### What a site host serves

All under one reserved prefix, so the rest of the namespace is the site's:

| Path | What |
|---|---|
| `/…` | The current version's files (`/` is `index.html`). |
| `/_clax/v/<n>/…` | Version `n`'s files, for review of older versions. |
| `/_clax/draft/<agent>/<token>/…` | A draft revision (its token stays its capability, as today, `crates/clax-server/src/routes/drafts.rs`). |
| `/_clax/blob/<asset id>` | An asset of this artifact. |
| `/_clax/api/…` | The page runtime's backend (below). |
| `/_clax/session`, `/_clax/signin` | The access handshake (below). |
| `/_clax/bridge/…`, `/_clax/…` UI files | The bridge and its parts. |
| `/_clax/healthz` | Health. |

- Publishing refuses files under `_clax/` (`crates/clax-core/src/publish.rs`
  path validation). Today `/_clax/`, `/_blob/`, `/healthz` and `/v/` would
  shadow published files.
- An asset is served only on the host of the artifact that holds it, at
  `/_clax/blob/<id>`; the next page contract version says so (the current
  one, `web/contract/0.2.61/assets.d.ts`, promises `"/_blob/" + id`).
- **Legacy artifacts.** Artifacts that exist when this lands are marked
  legacy (a flag the migration sets; new artifacts never have it). On a
  legacy artifact's host, `/_blob/<id>` is also reserved and serves that
  artifact's assets, so its existing pages and db documents keep working;
  publishing to it also refuses files under `_blob/`. Old keyed asset URLs
  (`<content origin>/_blob/<id>?k=…`) redirect to the owning artifact's host,
  where the access check applies. A page that used another artifact's asset
  breaks. New artifacts reserve `/_clax/` alone: the rest of the path space
  is theirs.
- The current version's pages are served `no-cache` with an ETag; versioned
  (`/_clax/v/<n>/`) files may stay immutable.
- Pages keep relative links (the page contract already requires them,
  `plugins/clax/skills/clax/SKILL.md`), so a version under `/_clax/v/<n>/`
  resolves its own files. Root-relative links in an older version reach the
  current version's files; that is accepted.
- Older versions and drafts share the site's origin and so its storage. A
  reviewer looking at version 3 sees the data the live site writes. Accepted:
  one origin per artifact is the isolation unit.

### Access

Each site request is decided, on every request, by the artifact's access:

- **Open to the world:** anyone may load the site and use the runtime as a
  visitor, anonymously. No handshake.
- **Otherwise (private, shared with people, shared by link):** the request
  must carry the site cookie, which names a **site session**. The server
  looks it up and checks the session's person or guest against the
  artifact's rights *now*. No session or no right means no content.

**The site session** is a server-side row (on the instance), bound to one
artifact host and to one of: a person, a guest, or an anonymous **link
viewer**. A link viewer is someone with no clax.dev session who opened a
page shared by link, as today (`crates/clax-http/src/hosted.rs`
`session_reads`): they keep reading without signing in or naming
themselves, with the link's rights, which do not reach the db (as today,
`crates/clax-store/src/hosted.rs` `LinkAccess::rights`). The session
re-checks the link's sharing on every request, so turning the link off ends
it. It has a short expiry (one hour, renewed by use, and ending when the
person's browser session ends). The cookie holds only an opaque session ID:

- `__Host-clax_site`, `Secure`, `HttpOnly`, `Path=/`, host-only.
- Top-level visits: `SameSite=Lax`, unpartitioned.
- Inside clax.dev's frame (a third-party context to clax.run): `SameSite=None;
  Partitioned` (CHIPS), keyed to clax.dev as top-level site, so no other
  top-level site can make the browser send it. Where a browser refuses the
  partitioned cookie (the frame's `/_clax/session` redirect arrives without
  it), the frame asks for storage access (the Storage Access API) and, if
  that is refused too, shows "Open in a tab", which takes the top-level
  path. The plan confirms current support in Chrome, Firefox and Safari
  and tests the fallback.

**The handshake** never puts a credential in a URL that lasts:

- *Framed:* the clax.dev shell, which knows the viewer, asks its own API
  for a one-use code for this artifact (`POST /api/artifacts/<id>/site-code`,
  sixty seconds, single use, bound to the session) and sets the frame's `src`
  to `https://<id>.clax.run/_clax/session?code=<c>&next=<path>`. The site
  host redeems the code with the instance, sets the partitioned cookie and
  redirects to `next`.
- *Top-level:* a private site request without a valid cookie redirects to
  `https://clax.dev/site-signin?host=<id>.clax.run&next=<path>`, which signs
  the visitor in to clax.dev if needed, checks the right, mints a code and
  redirects back to `/_clax/session`, which sets the unpartitioned cookie.
  A visitor without the right sees clax.dev's ordinary "no access" page.
- The code is the only thing in a URL. It is redeemed once, expires in
  seconds, and is sent with `Referrer-Policy: no-referrer`.
- *Every code is bound to a `state`* the visitor's own browser holds as a
  cookie on the site host (`clax_site_state`, path `/`, ten
  minutes), so an attacker cannot push a victim into the attacker's
  session (login CSRF). Top-level, the site host's redirect to
  `/site-signin` sets it and passes `state`; framed, the frame first loads
  `/_clax/session/start`, which sets it partitioned and posts it to the
  shell (`clax:site-state`, to the public origin), and the shell passes it
  to `site-code`. `/_clax/session` redeems only with the matching cookie.

**Opening to the world** is a new per-artifact state
(`artifacts.site_access`: `private` or `world`), set by whoever may manage
the artifact. An agent asking to open one to the world goes through the
person's approval, as `publish_public` does today. Closing it takes effect
on the next request. Artifacts that declare `db`, `assets` or `comments` may
be opened to the world; the next contract version drops the current
contract's "organization-internal and cannot be shared publicly" for them
(`web/contract/0.2.61/db.d.ts`, `assets.d.ts`, `comments.d.ts`).

**People on a world-open site.** A visitor arriving directly is anonymous.
clax.dev's frame, and its "Open site" link, always run the handshake, so a
person with rights (the owner administering their own app) acts with their
own level there. A world-open site offers no visible sign-in of its own.

### The runtime on the artifact's origin

Today every capability runs in the clax.dev shell: the page asks over
`postMessage`, the shell calls `/api/artifacts/<aid>/…` with its own cookie
(`web/shell/src/caps/db.ts`), and a top-level page gets `null` for every
capability (`web/bridge/src/bridge.ts`). That changes:

- The bridge talks to its own origin's `/_clax/api/…`, framed or top-level,
  with the site cookie (or as a world visitor). It no longer needs the shell
  for capabilities. The shell keeps a `postMessage` channel to the frame for
  what is the review UI's (comment anchors, live pointers, the room's
  cursors drawn over the page).
- `/_clax/api/` is scoped by its host to one artifact. No artifact ID in
  its paths; none could reach another.
- **Who the caller is:** a person or guest (from the site session), with the
  rights the artifact's clax.dev access gives them, or an anonymous
  **visitor** (world-open only). The `db` rules
  (`crates/clax-core/src/db.rs` `Rules`, levels `view` < `interact` < `admin`
  < `owner`) gain a level below `view`, `public`, which a world visitor
  meets. With no rule granting `public`, a visitor reads and writes nothing:
  the artifact's own declaration says what strangers may do. The default
  rules (no declaration) grant `public` nothing.
- A world visitor gets a stable anonymous visitor ID per browser: an
  `HttpOnly` cookie on the artifact's host alone, minted on first visit. It
  is their `{self}` for `data/users/{self}`, their author
  (`Author::Unconnected`), their presence in the room, and the key of the
  per-visitor limits. It identifies a browser, not a person, and an app may
  not treat it as more.
- Capabilities that are the review UI's (comments, threads) are not offered
  to world visitors: the clax.dev page has its own permissions.
- Writes to `/_clax/api/` require `Origin` equal to the artifact's own
  origin (CSRF); the API sends no `Access-Control-Allow-Origin`.
- World visitors are rate limited per client address (`CF-Connecting-IP`)
  and per artifact; the existing per-artifact caps (`MAX_DOCS`,
  `MAX_DOC_BYTES`, assets' sizes) bound what they can store.

### Each artifact's data in a store of its own

- App data (the `db` documents, leases, the document sequence, document
  authors, and the per-document `doc.write` audit records) moves out of the
  instance database into a per-artifact **artifact store**:
  - on the Worker, a Durable Object per artifact (a new SQLite-backed class,
    named by artifact ID);
  - locally, one SQLite file per artifact in the home
    (`~/.clax/artifacts/<id>/data.sqlite` or similar), with its own
    migrations list.
- The instance database keeps no app documents. The artifact store holds
  nothing of the instance's: no people, sessions, rights or threads. The
  rights decision is made by the instance and passed to the artifact store
  as the caller's resolved level for the request; the artifact store applies
  the artifact's rules to it.
- Agents' `db_*` tools and the clax.dev page read and write the same
  artifact store: there is one copy of an app's data.
- Existing documents move once: a migration (the next free number in
  `crates/clax-store/src/migrations.rs`) on the local daemon; on the Worker,
  each artifact store pulls its rows from the instance the first time it
  is opened and the instance deletes them once copied. Idempotent; no data
  lost; the instance's `docs` tables are dropped once empty.

### Headers on a site host

- `Content-Security-Policy: frame-ancestors 'self' <public origin>` (no
  other site may frame it); the snapshot policy's hard-coded `http://` and
  loopback (`crates/clax-server/src/routes/content.rs:56`, `:88`) take the
  real scheme and origins.
- `Cross-Origin-Resource-Policy: same-origin` on private content; world-open
  content `cross-origin` (a public site's images may be embedded).
- `Strict-Transport-Security` on clax.run with `includeSubDomains`.
- No `Access-Control-Allow-Origin` on private content.
- The sandboxed-frame mode and its CSP `sandbox` are gone for hosted
  content: the origin is the isolation.

### The shell and the bridge

- The shell frames `https://<id>.clax.run/…` (via the handshake for private
  artifacts) instead of the content origin; `frameOrigin` in
  `web/shell/src/bridge-link.ts` is the artifact's origin, never `"null"`.
- `artifactOrigin` (`web/shell/src/origin.ts`) and the bridge's
  `shellOrigins` (`web/bridge/src/channel.ts`) derive from the configured
  site domain and public origin, not from `localhost`.
- The `clax_frame` cookie's mode choice (`crates/clax-server/src/shell/boot.rs`
  `frame_origin`) goes: subdomain mode is the only mode.

### The daemon: local, and in hosted mode

There are three deployments of the one server: the local daemon, the
daemon in hosted mode (`[hosted] public_origin`, `docs/usage.md` "hosted
mode"), and the Worker. All three run this model.

- **Local:** `http://<id>.localhost:<port>`, site routes at the root,
  `/_clax/…` reserved, the handshake against the local shell (the owner's
  browser), the open-to-the-world state (on a local daemon it means "any
  local client without a handshake"), the runtime on the artifact's origin,
  and a SQLite file per artifact.
- **LAN viewing is removed.** Today a LAN viewer reaching the daemon by IP
  reads artifacts in a sandboxed frame (`docs/usage.md`, "A LAN viewer who
  is not connected can read artifacts and comment";
  `crates/clax-server/src/shell/boot.rs` `frame_origin`). With subdomain mode
  the only mode and `<id>.localhost` resolving only on the machine, a local
  daemon serves this machine alone; its docs and tests say so.
- **Daemon in hosted mode:** `[hosted] content_origin` is replaced by
  `[hosted] site_domain`; the self-hoster provides the wildcard DNS and
  certificate, as clax.dev does on Cloudflare. Its tests
  (`crates/clax-server/tests/api_hosted.rs`) move to the new model and
  carry most of the access and isolation coverage, since they run natively
  and fast.

### Cloudflare and the domain

- The clax.run zone on the same account; the Worker on the route
  `*.clax.run/*` (custom domains cannot be wildcards); a proxied wildcard DNS
  record; a certificate covering `*.clax.run` (Universal SSL covers the
  zone's first-level wildcard when clax.run is a zone of its own; Advanced
  Certificate Manager or Total TLS otherwise; the deploy's TLS check
  decides).
  `deploy.mjs` takes `--site-domain clax.run`, generates the route, and
  creates or checks the DNS record and certificate; what it cannot do
  through wrangler it prints as a step with a check that it was done.
- **Public Suffix List.** clax.run is submitted to the PSL (private section)
  so browsers treat each `<id>.clax.run` as a site of its own: without it,
  sibling artifacts are same-site, one can set `Domain=clax.run` cookies the
  others receive, and `SameSite` does not separate them. The submission is
  external and slow; until it lands the host-only `__Host-` cookie names
  keep the platform's cookies safe, and the remaining sibling risk is
  recorded as an accepted risk.
- The deploy record and `setup.env` gain the site domain;
  `smoke-remote.mjs` checks a real site host: the handshake, a private
  artifact refused without it, a world-open one served, and isolation of two
  artifacts' storage and runtime.

## Constraints

- AGENTS.md: crate layering (`scripts/check-crate-layers.py`), no `unsafe`,
  no hard links, tests without sleeps or wall-clock budgets (injected clocks
  for the session and code expiries), contiguous migration numbers.
- The Worker is a single-instance model (`CLAX_INSTANCE`, one `InstanceDo`);
  artifact stores are new Durable Objects beside it, not new instances.
- `e2e-hosted.mjs` covers the hosted content origin today but CI does not run
  it; the new site host, handshake, world-open and isolation checks run in
  CI.

## Calls made in this design, for the owner to check

These were settled by the design's author, not by the owner:

- Older versions and drafts at `/_clax/v/<n>/` and `/_clax/draft/…` on the
  site's own origin, sharing its storage.
- Link viewers keep anonymous reading through a link-viewer site session.
- The framed handshake's fallback: the Storage Access API, then "Open in a
  tab".
- World visitors get an anonymous per-browser visitor ID; the default rules
  grant them nothing.
- clax.dev's frame and "Open site" always run the handshake; world-open
  sites have no visible sign-in.
- An agent opening an artifact to the world needs the person's approval.
- The daemon in hosted mode follows the same model (`site_domain`).
- Per-document audit records are written in the artifact store and
  mirrored, metadata only (path, version, op, hash), into the instance's
  journal, so the journal and a tool call's `produced` keep every event
  (`docs/contract.md`, "The journal").
- Existing documents move lazily on each artifact's first call; the local
  daemon also sweeps every artifact at start.
- Clips move to the public origin behind the reader's credential.
- Other sites may not frame an artifact.

## Left to the builder

- The artifact store's crate placement within the layering, its schema and
  migrations, and how the instance and the artifact store exchange the
  caller's level on the Worker (a request from the instance object to the
  artifact object, or a signed context).
- The handshake's exact routes and the site-session table's shape.
- How clips (thread screenshots, today `/_clip/<aid>/<tid>?k=`) are served:
  they are review data, so they move to the public origin behind the reader's
  own credential, and their keyed address is retired.
- The order of the work, provided no deploy ships a state where a private
  artifact is readable without the right.
