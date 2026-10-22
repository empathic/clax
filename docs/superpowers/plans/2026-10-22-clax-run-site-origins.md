# Each Artifact on Its Own Origin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Serve each artifact as a site of its own at `https://<id>.clax.run/`
(locally `http://<id>.localhost:<port>/`), readable only by those the
artifact's access allows, with its page runtime and its app data scoped to
that one artifact.

**Architecture:** One host-to-artifact lookup (`SiteHosts`) runs before every
route and turns a site host into a `SiteRequest`. Site routes serve the
current version at the root and everything of Clax's under `/_clax/`. A site
session (a server-side row, opaque cookie, made by a one-use code from the
shell) decides every private request; a new `site_access = world` state lets
anyone in. The runtime's backend is served on the site's own host
(`/_clax/api/…`), and app data moves from the instance database into a
store per artifact (`AppData`: a SQLite file locally, a Durable Object on the
Worker).

**Tech Stack:** Rust (axum, rusqlite), the Cloudflare Worker (`worker` crate,
Durable Objects), TypeScript (the shell and the bridge, Vitest, Playwright),
Node (`deploy.mjs`, `e2e-hosted.mjs`, `smoke-remote.mjs`).

**Spec:** `docs/superpowers/specs/2026-10-21-clax-run-site-origins-design.md`

## Global Constraints

- Site domain on clax.dev: `clax.run`; locally `localhost`. A site host is
  `<12-char lowercase ID>.<site domain>[:port]` (`clax_core::ArtifactId::parse`).
- Reserved on every site host: `/_clax/` alone. On a legacy artifact's host
  also `/_blob/`.
- Site cookie: `__Host-clax_site` on `https`, `clax_site` on `http`;
  `Secure` (https), `HttpOnly`, `Path=/`, host-only. Top-level
  `SameSite=Lax`; framed `SameSite=None; Partitioned`.
- Visitor cookie (world-open only): `__Host-clax_visitor` / `clax_visitor`,
  same attributes as the site cookie's top-level form, one year.
- Site session: one hour, renewed by use; site code: sixty seconds, one use.
- `Referrer-Policy: no-referrer` on every site response.
- `frame-ancestors 'self' <public origin>` on every site page.
- Next free migration on main: 42. This branch takes 42 onward,
  contiguously, and renumbers if another branch lands first.
- AGENTS.md: crate layering (`scripts/check-crate-layers.py`), no `unsafe`,
  no hard links, injected clocks (no sleeps, no wall-clock budgets), "ID" in
  prose, commits with `git -c commit.gpgsign=false commit`, Bazel through
  `just` (never bare `cargo test` in the worktree; `just test-native` if
  native is needed), `qualifier threads <path>` before editing a file.
- No deploy of a state where a private artifact is readable without the
  right: tasks 1–6 land together before any deploy.
- Test commands: Bazel is on PATH, so run tests through Bazel targets
  (`docs/bazel.md`): e.g. `bazelisk test //crates/clax-server:integration
  --test_arg=api_site`, `bazelisk test //crates/clax-http:clax-http_lib_test`,
  `just web-test`, `just web-e2e`, `just worker-test`. A `just test-native …`
  in a task below means the equivalent Bazel target, never native Cargo.
- Migration numbers (as merged): 42 `legacy_paths`, 43 `app_data_moved`
  and `app_data_folds`, 44 site sessions and `site_access`.

## Review Focus

1. A site host carrying another artifact's path (`<a>.clax.run/_clax/blob/<asset of b>`,
   `/_clax/api/` with a forged artifact ID in a body or query) must answer
   404: the host alone names the artifact. Test in Task 2 and Task 7.
2. A person whose right (or a share, or the link) is removed while their
   site session is live must be refused on their very next request, not at
   the session's expiry. Test in Task 3.
3. Paths that the Worker dispatches before looking at the host (`/git/`,
   `/_diag/`, `/_dev/repos/`, `/do/`, `/mcp`) must answer 404 on a site host.
   Test in Task 11.
4. A world-open artifact switched back to private must refuse an anonymous
   visitor on the next request, including its runtime calls and its assets.
   Test in Task 8.
5. A legacy artifact's old keyed asset URL must never serve bytes without
   the access check: it only redirects to the owning site host. Test in
   Task 4.

---

## Phase A: site hosts, access and the shell (server, natively)

### Task 1: The site domain and the host-to-artifact lookup

**Files:**
- Modify: `crates/clax-http/src/host.rs` (whole file: `artifact_host`,
  `OnArtifactOrigin`, `rewrite_artifact_host` become the items below)
- Modify: `crates/clax-core/src/config.rs` (`HostedConfig`: add `site_domain`)
- Modify: `crates/clax-http/src/state.rs` (`AppState`: add `sites: SiteHosts`)
- Modify: `crates/clax-server/src/router.rs:719-750` (`wrap`: the lookup is
  the outermost layer, as `rewrite_artifact_host` is today)
- Test: `crates/clax-http/src/host.rs` (unit tests), `crates/clax-server/tests/api_host.rs`

**Interfaces:**
- Produces:
  - `pub struct SiteHosts { domain: String }` with
    `pub fn local() -> SiteHosts` (domain `localhost`),
    `pub fn new(domain: &str) -> Option<SiteHosts>` (refuses an empty domain,
    one with a scheme, a port or a leading dot),
    `pub fn domain(&self) -> &str`,
    `pub fn target(&self, host: &str) -> Option<SiteTarget>`,
    `pub fn origin(&self, id: &ArtifactId, scheme: &str, port: Option<u16>) -> String`.
  - `pub struct SiteTarget { pub artifact: ArtifactId }` — the one place a
    vanity name or custom hostname will later resolve.
  - `#[derive(Clone)] pub struct SiteRequest { pub artifact: ArtifactId, pub origin: String }`
    request extension (replaces `OnArtifactOrigin`).
  - `pub async fn site_layer(State(s): State<AppState>, req, next) -> Response`:
    on a site host inserts `SiteRequest` and leaves the path unchanged
    (Task 2 mounts the site routes; nothing is rewritten to `/c/…`).
  - `HostedConfig.site_domain: Option<String>` (`[hosted] site_domain`).

- [ ] **Step 1: Write the failing unit tests** in `host.rs`:
  `target_accepts_an_id_under_the_domain_with_or_without_a_port`
  (`SiteHosts::new("clax.run").target("abcdefghjkmn.clax.run")` and
  `…clax.run:443` → `Some`, artifact `abcdefghjkmn`);
  `target_refuses_nested_uppercase_and_foreign_hosts`
  (`a.b.clax.run`, `ABCDEFGHJKMN.clax.run`, `abcdefghjkmn.clax.run.evil.com`,
  `clax.run`, `abcdefghjkmn.clax.dev` → `None`);
  `local_hosts_keep_the_localhost_form` (`SiteHosts::local()` accepts
  `abcdefghjkmn.localhost:7480`, refuses `abcdefghjkmn.localhost:` and
  `abcdefghjkmn.localhost:x`, the cases of today's
  `accepts_only_anchored_lowercase_ids`).
- [ ] **Step 2: Run** `just test-native -p clax-http host::` — expect FAIL (no `SiteHosts`).
- [ ] **Step 3: Implement** `SiteHosts`, `SiteTarget`, `SiteRequest`,
  `site_layer`; keep `artifact_host` as `SiteHosts::local().target(h).map(|t| t.artifact)`
  until Task 2 removes its last caller. `AppState` builds `sites` from
  `HostedConfig.site_domain`, else `SiteHosts::local()`.
- [ ] **Step 4: Run** the unit tests and `crates/clax-server/tests/api_host.rs` — expect PASS.
- [ ] **Step 5: Commit** `git -c commit.gpgsign=false commit -m "Look up a site host's artifact through one configured site domain"`.

### Task 2: Site routes: the current version at the root, Clax's under `/_clax/`

**Files:**
- Create: `crates/clax-server/src/routes/site.rs` (the site router)
- Modify: `crates/clax-server/src/router.rs:470-510` (mount the site router for
  `SiteRequest` requests, before the instance's own routes; remove
  `/c/{aid}/…` routes once Task 5 stops the shell using them)
- Modify: `crates/clax-server/src/routes/content.rs` (serve a version's file by
  `(artifact, n, path)`; the current version with `no-cache` and an ETag)
- Modify: `crates/clax-server/src/routes/drafts.rs` (serve at `/_clax/draft/…`)
- Modify: `crates/clax-server/src/routes/assets.rs` (`/_clax/blob/<id>`, only for
  the host's artifact)
- Modify: `crates/clax-core/src/publish.rs:123` (`check_path`: refuse a first
  segment `_clax`; refuse `_blob` when the artifact is legacy)
- Modify: `crates/clax-store/src/migrations.rs` (migration 42:
  `ALTER TABLE artifacts ADD COLUMN legacy_paths INTEGER NOT NULL DEFAULT 0;
  UPDATE artifacts SET legacy_paths = 1;`)
- Test: `crates/clax-server/tests/api_site.rs` (new, added to the crate's
  `tests/integration.rs`)

**Interfaces:**
- Consumes: `SiteRequest` (Task 1).
- Produces:
  - `pub fn site_routes() -> Router<AppState>` with:
    `GET /` and `GET /{*path}` (current version),
    `GET /_clax/v/{n}/` and `/_clax/v/{n}/{*path}`,
    `GET /_clax/draft/{agent}/{token}/` and `…/{*path}`,
    `GET /_clax/blob/{asset_id}`, `GET /_clax/healthz`,
    `GET /_clax/{*path}` (UI files and the bridge, as today's `shell::static_file`),
    and on a legacy artifact `GET /_blob/{asset_id}`.
    Every other `/_clax/…` path is 404.
  - `pub fn check_path_for(path: &str, legacy: bool) -> Result<()>` in
    `publish.rs` (today's `check_path` plus the reserved first segments;
    `invalid_path` with message `'_clax/…' is reserved`).
  - `ArtifactRow.legacy_paths: bool` in `clax-store`.

- [ ] **Step 1: Write the failing tests** in `api_site.rs` (a local
  `TestServer`, requests with `Host: <id>.localhost:<port>`, access not yet
  enforced — Task 3 adds it):
  - `the_root_serves_the_current_version`: publish v1 `index.html` "one",
    v2 "two"; `GET /` → 200 "two", `Cache-Control: no-cache`, an `ETag`.
  - `an_older_version_is_served_under_clax_v`: `GET /_clax/v/1/` → "one".
  - `a_published_path_under_clax_is_refused`: publishing `_clax/x.js` → 400
    `invalid_path`.
  - `a_new_artifact_owns_blob_paths`: a new artifact publishes `_blob/a.txt`
    and `GET /_blob/a.txt` serves it.
  - `a_legacy_artifact_keeps_blob_reserved`: an artifact with
    `legacy_paths = 1` serves its asset at `/_blob/<id>` and refuses
    publishing `_blob/a.txt`.
  - `an_asset_is_served_only_on_its_own_artifacts_host`: asset of artifact B
    at `<a>.localhost/_clax/blob/<b asset>` → 404; at B's host → 200.
  - `an_unknown_clax_path_is_404`: `GET /_clax/nope` → 404.
  - `the_public_host_serves_no_content`: `GET /c/<id>/v/1/` on the public
    host → 404.
- [ ] **Step 2: Run** `just test-native -p clax-server --test integration api_site` — expect FAIL.
- [ ] **Step 3: Implement** the site router, the migration, `check_path_for`
  (called from publish with the artifact's `legacy_paths`).
- [ ] **Step 4: Run** the same command and `api_content`, `api_assets`,
  `api_drafts` — expect PASS (update their URLs from `/c/<id>/v/<n>/` and
  `<id>.localhost/v/<n>/` to the site forms).
- [ ] **Step 5: Commit** `"Serve each artifact's site from its host's root, with Clax's paths under /_clax/"`.

### Task 3: Site sessions, the handshake and the access check

**Files:**
- Create: `crates/clax-store/src/site_sessions.rs`
- Create: `crates/clax-http/src/site_access.rs` (the access layer and cookies)
- Modify: `crates/clax-store/src/migrations.rs` (migration 44, below)
- Modify: `crates/clax-server/src/routes/site.rs` (`/_clax/session`)
- Modify: `crates/clax-server/src/routes/artifacts.rs` (`POST /api/artifacts/{aid}/site-code`)
- Create: `crates/clax-server/src/routes/site_signin.rs` (`GET /site-signin` on the public host)
- Test: `crates/clax-server/tests/api_site_access.rs`

Closes ad4d8d8c (with Task 4, which removes the keys).

**Interfaces:**
- Consumes: `SiteRequest` (Task 1), `site_routes` (Task 2).
- Produces:
  - Migration 44:
    ```sql
    CREATE TABLE site_codes (code_hash TEXT PRIMARY KEY, artifact_id TEXT NOT NULL,
      holder_kind TEXT NOT NULL CHECK (holder_kind IN ('person','guest','link','owner')),
      holder TEXT, browser_session TEXT, partitioned INTEGER NOT NULL,
      expires_at TEXT NOT NULL);
    CREATE TABLE site_sessions (id_hash TEXT PRIMARY KEY, artifact_id TEXT NOT NULL,
      holder_kind TEXT NOT NULL, holder TEXT, browser_session TEXT,
      expires_at TEXT NOT NULL, created_at TEXT NOT NULL);
    CREATE INDEX site_sessions_by_browser ON site_sessions(browser_session);
    ALTER TABLE artifacts ADD COLUMN site_access TEXT NOT NULL DEFAULT 'private'
      CHECK (site_access IN ('private','world'));
    ```
  - `clax_store::site_sessions`:
    `pub enum Holder { Owner, Person(String), Guest(String), Link }`,
    `impl Store { pub fn site_code_mint(&self, aid: &ArtifactId, holder: &Holder, browser_session: Option<&str>, partitioned: bool) -> Result<String>;
    pub fn site_code_redeem(&self, code: &str, aid: &ArtifactId) -> Result<Option<(String /*session ID*/, Holder, bool /*partitioned*/)>>;
    pub fn site_session(&self, id: &str, aid: &ArtifactId) -> Result<Option<Holder>>;` (renews `expires_at` by one hour on use; `None` when expired, for another artifact, or its browser session ended) `}`.
    Codes and session IDs are 32 random bytes, base64url; only their SHA-256 is stored.
  - `clax_http::site_access`:
    `pub async fn site_access_layer(State, req, next)` (after `site_layer`):
    world-open → passes as `SiteCaller::Visitor`; else reads the site
    cookie, resolves `Holder`, checks the holder's read right on the
    artifact *now* (the same check `GET /api/artifacts/<id>` makes, and for
    `Holder::Link` that the artifact is still shared by link), inserts
    `SiteCaller::Holder(Holder)`; else answers: top-level navigation →
    302 to `<public origin>/site-signin?host=<host>&next=<path>`; a
    subresource or API call → 401 `site_session_required`.
    `pub enum SiteCaller { Holder(Holder), Visitor(String /*visitor ID*/) }`.
    `/_clax/healthz` (and the bridge's static `/_clax/` files) stay outside
    the check: `/_clax/healthz` answers 200 with `/healthz`'s JSON on any
    well-formed site host, whether or not the artifact exists, and reveals
    nothing about it (the deploy probes a random ID).
    `pub fn site_cookie(secure: bool, partitioned: bool, value: &str) -> HeaderValue`.
  - `POST /api/artifacts/{aid}/site-code` (public host, the shell's own
    credential) → `{"src": "<site origin>/_clax/session?code=<c>&next=<path>"}`;
    body `{"next": "/_clax/v/3/", "partitioned": true}`; 404 when the caller
    may not read the artifact.
  - `GET /_clax/session?code=&next=` on a site host: redeems, sets the
    cookie (partitioned per the code), 302 to `next` (a path on this host
    only; anything else becomes `/`).
  - `GET /site-signin?host=&next=` on the public host: unknown host → 404;
    signed out and not shared by link → 302 to `/signin?next=…`; no right →
    the ordinary no-access page; else mints an unpartitioned code and 302s
    to the site's `/_clax/session`. Sends `Referrer-Policy: no-referrer`.

- [ ] **Step 1: Write the failing tests** in `api_site_access.rs` (a hosted
  `TestServer::spawn_hosted` with `site_domain = "localhost"` and a
  `ManualClock`):
  - `a_private_site_without_a_session_redirects_to_site_signin` (navigation
    `Accept: text/html` → 302 to `<public>/site-signin?host=…&next=%2F`).
  - `a_private_sites_subresource_without_a_session_is_401`.
  - `the_shell_code_signs_the_frame_in_and_the_cookie_is_partitioned`
    (owner cookie → `POST site-code` → `GET src` → `Set-Cookie` contains
    `HttpOnly`, `SameSite=None`, `Partitioned`; then `GET /` with it → 200).
  - `a_code_is_single_use_and_expires_after_sixty_seconds` (second redeem →
    no cookie, 302 to site-signin; a fresh code after `clock.advance(61s)` →
    same).
  - `revoking_a_share_ends_the_site_session_on_the_next_request` (member B
    shared, session made, share removed, next `GET /` → refused).
  - `a_link_viewer_reads_without_signing_in_until_the_link_is_turned_off`.
  - `a_site_session_is_for_one_artifact` (A's cookie on B's host → refused).
  - `a_session_expires_after_an_hour_unused_and_renews_by_use`.
  - `next_never_leaves_the_site_host` (`next=https://evil.example/` → 302 to `/`).
- [ ] **Step 2: Run** `just test-native -p clax-server --test integration api_site_access` — expect FAIL.
- [ ] **Step 3: Implement** the store module, the layer, the three routes.
  The local daemon runs the same layer: its owner's shell token is the
  owner's credential for `site-code`.
- [ ] **Step 4: Run** it and `api_hosted` — expect PASS.
- [ ] **Step 5: Commit** `"Decide every private site request by a site session the shell or /site-signin makes"`.

### Task 4: Retire content keys and the content origin; legacy redirects; clips

**Files:**
- Modify: `crates/clax-http/src/hosted.rs` (remove `content`, `with_content_origin`,
  `content_origin`, `is_content_host`, `content_key`, `content_path`,
  `asset_key`, `stamp_content_keys`, the content branch of `gate`; keep
  `clip_key` only until the clip route below replaces it)
- Modify: `crates/clax-server/src/shell/boot.rs:280-330` (frame `src` from Task 5's code route)
- Modify: `crates/clax-server/src/routes/assets.rs` (`asset_url` returns
  `/_clax/blob/<id>` for new artifacts, `/_blob/<id>` for legacy ones, both
  relative to the site host)
- Modify: `crates/clax-server/src/routes/threads.rs` (`keyed_clip` → `GET /api/artifacts/{aid}/threads/{tid}/clip`,
  the reader's own credential)
- Modify: `crates/clax-testkit/src/server.rs:412-424` (`with_content_origin` → `site_domain`)
- Test: `crates/clax-server/tests/api_hosted.rs` (rewrite
  `content_is_served_only_at_its_key_and_an_id_alone_reads_nothing`,
  `with_a_content_origin_*`), `api_site_access.rs`

Closes ad4d8d8c (with Task 3).

**Interfaces:**
- Consumes: `SiteHosts::origin` (Task 1), `site_access_layer` (Task 3).
- Produces: `GET <public>/_blob/<id>?k=…` and `GET <old content host>/_blob/<id>?k=…`
  → 301 to `<owning site origin>/_blob/<id>` (legacy) with no body; the
  site host then applies the access check. `/c/…/k/…` → 404.

- [ ] **Step 1: Write the failing tests:**
  `an_old_keyed_asset_url_only_redirects_to_its_site` (301, `Location` is the
  owning host's `/_blob/<id>`, no bytes; following it without a session →
  302 to site-signin);
  `an_old_keyed_page_url_is_gone` (`/c/<id>/v/1/k/<key>/` → 404);
  `a_clip_is_served_to_its_reader_alone` (owner 200, signed out 401,
  member without the right 404).
- [ ] **Step 2: Run** — expect FAIL.
- [ ] **Step 3: Implement** the removals and the redirect (the redirect
  route looks up the asset's artifact; an unknown asset → 404).
- [ ] **Step 4: Run** `api_hosted`, `api_site_access`, `api_assets`,
  `api_threads` — expect PASS.
- [ ] **Step 5: Commit** `"Retire keyed content addresses: old asset URLs redirect to their site, clips need the reader's credential"`.

### Task 5: The shell frames the site origin; the bridge trusts the configured shell; LAN viewing goes

**Files:**
- Modify: `crates/clax-server/src/shell/boot.rs:81-150, 299-319` (remove
  `frame_origin` and the `clax_frame` cookie; boot JSON `frame: {mode: "site",
  origin: "<site origin>"}`; no `src`: the shell asks for a code)
- Modify: `web/shell/src/origin.ts` (`artifactOrigin(id, boot)` from the boot's
  `site.domain`, `site.scheme`, `site.port`; remove `probeOrigin`)
- Modify: `web/shell/src/view/artifact-controller.ts:917-950` (`decideOrigin`
  removed; the frame `src` comes from `POST /api/artifacts/<id>/site-code`)
- Modify: `web/shell/src/view/frame-host.ts:65` (no `sandbox` attribute; add
  `allow="storage-access"`)
- Modify: `web/shell/src/bridge-link.ts:7-34` (`frameOrigin` is the site
  origin, never `"null"`)
- Modify: `web/bridge/src/channel.ts:10-15` (`shellOrigins` from the tag's
  `data-origin`, always set by `wrap.rs`, never from `location`)
- Modify: `crates/clax-core/src/wrap.rs:183-203` (always write `data-origin`)
- Modify: `crates/clax-server/src/routes/content.rs:41-91` (`content_policy`:
  `frame-ancestors 'self' <public origin>`; `snapshot_policy` takes the
  request's scheme)
- Create: `web/shell/src/view/open-in-tab.svelte` (the fallback notice)
- Modify: `crates/clax-http/src/auth.rs` and `identity.rs` (a local daemon
  admits loopback only: a LAN viewer's request → 403 `forbidden_host`)
- Modify: `docs/usage.md:727` (LAN viewing removed)
- Test: `web/shell/src/origin.test.ts`, `web/shell/src/frame-src-cases.json`,
  `web/e2e/boot.spec.ts`, `web/e2e/site-frame.spec.ts` (new),
  `crates/clax-server/tests/api_content.rs`

Closes 0a1a0d4b, 49249c3c, 18483473 by removing LAN viewers (with evidence
from the test below; the owner confirms the closes).

**Interfaces:**
- Consumes: `POST /api/artifacts/{aid}/site-code` (Task 3).
- Produces: boot field `site: {domain: string, scheme: "http"|"https", port: number|null}`;
  the frame `src` is the code route's `src`. When the frame's first load
  reports `clax:no-cookie` (the bridge sees `document.hasStorageAccess()`
  false and its first `/_clax/api/whoami` 401), the shell calls
  `requestStorageAccess` through the frame, then shows `open-in-tab`.

- [ ] **Step 1: Write the failing tests:**
  `site-frame.spec.ts`: "the frame loads the site origin unsandboxed and
  its localStorage is the artifact's own" (two artifacts write
  `localStorage.x`; each reads back its own);
  "no other origin can frame a site page" (CSP `frame-ancestors`);
  `origin.test.ts`: `artifactOrigin` for `clax.run`/`https`/`null` →
  `https://<id>.clax.run`; `api_content.rs`:
  `a_lan_viewer_is_refused` (peer `192.168.1.2` → 403).
- [ ] **Step 2: Run** `just web-test` and the e2e spec — expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `just web-test`, `just web-e2e` (the Playwright lane),
  `api_content` — expect PASS.
- [ ] **Step 5: Commit** `"Frame each artifact on its own site origin through a site code; remove LAN viewing"`.

### Task 6: Site response headers

**Files:**
- Modify: `crates/clax-server/src/routes/site.rs` (one `site_headers` layer)
- Test: `crates/clax-server/tests/api_site.rs`

**Interfaces:**
- Produces: on every site response `Referrer-Policy: no-referrer`,
  `X-Content-Type-Options: nosniff`, `Cross-Origin-Resource-Policy:
  same-origin` (private) / `cross-origin` (world); on `https`
  `Strict-Transport-Security: max-age=31536000; includeSubDomains`; never
  `Access-Control-Allow-Origin` on a private artifact.

- [ ] **Step 1: Write the failing test** `site_responses_carry_the_isolation_headers`
  (private page, private asset, world page: each header above).
- [ ] **Step 2–4:** run, implement, run — FAIL then PASS.
- [ ] **Step 5: Commit** `"Send the isolation headers on every site response"`.

## Phase B: the runtime on the site's origin

### Task 7: `/_clax/api/` on the site host, scoped by the host

**Files:**
- Create: `crates/clax-server/src/routes/site_api.rs`
- Modify: `crates/clax-core/src/db.rs:25-60` (`Level::Public` below `View`;
  `"public"` in `parse`/`as_str`; default rules grant it nothing)
- Modify: `crates/clax-http/src/db_caller.rs` (a `SiteCaller` resolves to a
  `Caller`: holder → its rights' level as today; visitor →
  `Caller { level: Level::Public, viewer: Some(visitor ID), person: Author::Unconnected, via_page: true }`)
- Modify: `crates/clax-http/src/site_access.rs` (mint the visitor cookie on a
  world-open site's first response)
- Modify: `crates/clax-http/src/hosted.rs` `Limits` (new `Attempt::SiteWrite`,
  per client address and per artifact)
- Test: `crates/clax-server/tests/api_site_api.rs`

Closes 94ab5cda (the `user` capability is answered by the server from the
site caller, not computed from the shell token).

**Interfaces:**
- Consumes: `SiteCaller` (Task 3).
- Produces: under `/_clax/api/`, the same request and response bodies as
  today's `/api/artifacts/{aid}/…` routes, with no `{aid}`:
  `docs/{*path}` (GET, PUT, PATCH, DELETE), `docs` (query), `docs:batch`,
  `docs:acquire`, `assets` (POST, GET list), `room` (WebSocket),
  `artifact` (GET), `whoami` (GET: `{kind: "owner"|"person"|"guest"|"link"|"visitor", name?, can_edit}`).
  Comments and threads are not served to a `Visitor`. Every write needs
  `Origin` equal to the site origin (403 `forbidden_origin`). Each handler
  calls the existing route function with the host's artifact ID; no path,
  query or body field names an artifact.

- [ ] **Step 1: Write the failing tests:**
  `the_api_reaches_only_the_hosts_artifact` (a body or query naming
  artifact B on A's host changes nothing of B);
  `a_visitor_reads_and_writes_only_what_public_rules_allow`
  (rules `[{path: "data/guestbook", read: "public", write: "public"}]`;
  visitor writes there 200, elsewhere 404);
  `a_visitor_has_a_self_subtree` (two visitor cookies, `data/users/{self}`
  separate);
  `a_write_from_another_origin_is_refused`;
  `whoami_names_the_caller`;
  `visitor_writes_are_rate_limited` (with the limits' clock).
- [ ] **Step 2–4:** run (`api_site_api`), implement, run — FAIL then PASS;
  also `clax-core` `db::` unit tests for `Level::Public`.
- [ ] **Step 5: Commit** `"Serve the page runtime on the site's own host, scoped to its artifact, with a public level for visitors"`.

### Task 8: Opening an artifact to the world

**Files:**
- Modify: `crates/clax-server/src/routes/artifacts.rs` (`PATCH /api/artifacts/{aid}`
  accepts `site_access: "private"|"world"`; needs the manage right)
- Modify: `crates/clax-mcp/src/tools.rs` (a `site_access` argument on the
  metadata tool; `world` goes through the person's approval as
  `publish_public` does, `crates/clax-mcp/tests/publish_public.rs`)
- Modify: `web/shell/src/view/` share dialog (an "Open to the world" toggle
  with the site's URL)
- Modify: `web/contract/` (new contract version: `db.d.ts`, `assets.d.ts`,
  `comments.d.ts` drop "organization-internal and cannot be shared
  publicly"; `assets.d.ts` names `/_clax/blob/<id>`; the `public` level)
- Modify: `plugins/clax/skills/clax/SKILL.md` and its copies (the contract
  words; `scripts/test-plugins.sh` checks they match `docs/contract.md`)
- Test: `crates/clax-server/tests/api_site_access.rs`, `crates/clax-mcp/tests/site_access.rs`

**Interfaces:**
- Consumes: `artifacts.site_access` (Task 3), `SiteCaller::Visitor` (Task 7).

- [ ] **Step 1: Write the failing tests:**
  `a_world_site_is_served_to_anyone_and_its_page_is_not`
  (signed out: site `GET /` 200; `GET <public>/a/<id>` still sign-in);
  `closing_a_world_site_refuses_the_next_visitor_request`
  (page, asset and `/_clax/api/docs` each refused right after);
  `an_agent_opening_to_the_world_waits_for_approval` (MCP test);
  `the_open_site_link_signs_the_owner_in_on_a_world_site` (the shell's
  "Open site" goes through `/site-signin`; on a world site a valid site
  cookie makes the caller its holder, not a visitor: `whoami` → `owner`).
- [ ] **Step 2–4:** run, implement, run — FAIL then PASS.
- [ ] **Step 5: Commit** `"Let an artifact's manager open its site to the world"`.

### Task 9: The bridge calls its own origin

**Files:**
- Create: `web/bridge/src/caps/origin-client.ts` (`fetch` to `/_clax/api/…`
  with `credentials: "same-origin"`; the room's WebSocket to `/_clax/api/room`)
- Modify: `web/bridge/src/bridge.ts:95-137` (`makeUse` resolves every
  capability through the origin client, framed or top-level; `framed`
  only gates the shell channel)
- Modify: `web/shell/src/caps/registry.ts` and the moved handlers
  (`db.ts`, `assets.ts`, `artifact.ts`, `user.ts`, `room.ts`): their request
  building moves to the bridge's caps part; the shell keeps `permissions`,
  `downloads`, `sample` and the gesture prompts, reached over the channel
  when framed and unavailable (`null` from `use`) top-level
- Test: `web/bridge/src/caps/origin-client.test.ts`, `web/e2e/site-runtime.spec.ts` (new)

**Interfaces:**
- Consumes: `/_clax/api/…` (Task 7).
- Produces: `export function originClient(base: URL): { call(cap: string, method: string, args: unknown): Promise<unknown>; socket(path: string): WebSocket }`.

- [ ] **Step 1: Write the failing tests:**
  `site-runtime.spec.ts`: "a world-open site opened top-level writes and
  reads its db" and "a private site framed in the shell reads its db
  through its own origin" (assert no request from the frame goes to the
  public origin's `/api/`);
  `origin-client.test.ts`: a call never names an artifact ID.
- [ ] **Step 2–4:** run (`just web-test`, the e2e spec), implement, run — FAIL then PASS.
- [ ] **Step 5: Commit** `"Run the page runtime from the site's own origin, framed or top-level"`.

## Phase C: each artifact's data in a store of its own

### Task 10: `AppData`, the per-artifact store, locally

**Files:**
- Create: `crates/clax-store/src/app_data/mod.rs` (the store),
  `crates/clax-store/src/app_data/migrations.rs` (its own list)
- Modify: `crates/clax-store/src/docs.rs` (the document logic moves into
  `app_data`, parameterised by `&Rules`; `Store::doc_*` are removed)
- Modify: `crates/clax-core/src/home.rs` (`Home::app_data_path(&ArtifactId) -> PathBuf`
  = `<home>/artifacts/<id>/data.sqlite`)
- Create: `crates/clax-http/src/app_data_host.rs` (the trait and its local impl)
- Modify: `crates/clax-server/src/routes/docs.rs` (calls `AppDataHost`, then
  filters with the instance's `Vis` as today)
- Modify: `crates/clax-store/src/migrations.rs` (migration 44:
  `CREATE TABLE app_data_moved (artifact_id TEXT PRIMARY KEY)`; the
  instance's `docs`, `leases` and `doc_authors` tables stay, emptied
  artifact by artifact, and are dropped by a later release once every
  artifact has moved: record that as a concern on the migration)
- Test: `crates/clax-store/src/app_data/tests.rs`, `crates/clax-server/tests/api_docs.rs` (existing, must stay green)

**Interfaces:**
- Produces:
  - `pub struct AppData` with `pub fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<AppData>`,
    `pub fn open_on(conn: Connection, clock: Arc<dyn Clock>) -> Result<AppData>`,
    and the document operations of today's `Store::doc_*` with the same
    names and results, each taking `rules: &Rules` before `caller`
    (e.g. `pub fn doc_get_authored(&self, rules: &Rules, path: &str, caller: &Caller) -> Result<Option<(Doc, Vec<Author>)>>`),
    plus `pub fn import(&self, rows: Exported) -> Result<()>` and
    `pub fn audit_since(&self, seq: i64) -> Result<Vec<AuditRecord>>`.
    Its tables: `docs`, `leases`, `doc_authors`, `doc_seq` (one row), and
    `audit_events` with the instance's `doc.write` shape.
  - `Store::export_app_data(&self, aid: &ArtifactId) -> Result<Exported>`
    and `Store::forget_app_data(&self, aid) -> Result<()>` (used once per
    artifact by the move).
  - `#[async_trait] pub trait AppDataHost: Send + Sync { async fn call(&self, aid: &ArtifactId, op: AppDataOp) -> Result<AppDataReply>; }`
    with `AppDataOp`/`AppDataReply` serde enums mirroring each doc
    operation (the Worker serialises them, Task 11). `LocalAppData`
    implements it over `AppData::open(home.app_data_path(aid))`.
  - `pub struct MovingAppData<H: AppDataHost> { inner: H, store: Store }`,
    itself an `AppDataHost`, which every deployment wraps around its host:
    before an artifact's first call it exports the artifact's rows from the
    instance, sends `AppDataOp::Import`, marks `app_data_moved` and forgets
    them, in that order (a crash between import and forget re-imports the
    same rows, which `import` treats as already present).
  - Deleting an artifact deletes its `artifacts/<id>/` directory (locally
    `FsContent::delete_artifact` already does; the Worker's erase is Task 11).

**Where the instance touches app data beyond `Store::doc_*` (all in scope):**
- *Viewer claims* (`crates/clax-store/src/viewers.rs` `fold`:
  `fold_authorship`, `move_private_docs`) rewrite authors and
  `data/users/<old>/…` across every artifact inside the instance's
  transaction. Replace with a fold log: migration 44 also creates
  `app_data_folds(seq INTEGER PRIMARY KEY, old TEXT NOT NULL, new TEXT NOT NULL, into_owner INTEGER NOT NULL)`,
  written by `fold`. `AppData` keeps the last fold `seq` it applied;
  `MovingAppData` sends `AppDataOp::Fold { folds }` before an artifact's
  call when the log is ahead; `AppData` skips folds at or below its mark.
  `Exported` carries the fold `seq` read in the same snapshot as its rows,
  and `import` sets it.
- *Approvals* (`crates/clax-store/src/approvals.rs`
  `withheld_outside_threads`, `authored_outside_threads`; callers
  `crates/clax-server/src/routes/approvals.rs:432`,
  `crates/clax-http/src/views/approval.rs:134`): the callers fetch distinct
  authors, and the paths one author wrote, through the host first
  (`AppDataOp::Authors`, `AppDataOp::AuthoredBy`) and pass them in.
- *Visibility inside the scan* (`doc_query_visible`, `refuse_withheld`,
  `acquire`: withheld counts, gap-free paging, byte budgets): the instance
  projects the reader's `Visibility` onto one artifact as a serialisable
  `DocView { reader, owner, approved_keys, people: Option<…> }` that
  `AppData` applies in the scan; rules and the reader decision stay with
  the instance.
- *Page authors* (`artifacts::page_authors_in`, for `Caller.via_page`
  writes): the instance passes them with the op.
- *Wire types*: `Caller`, `Author`, `Rules`, `AuditCtx`, `AuditRecord` and
  `CoreError` are not serde. `AppDataOp`/`AppDataReply` carry mirror types:
  rules as the capabilities JSON, the caller as plain fields, the audit
  envelope pre-rendered (`audit::body_json`), and a wire error with a fixed
  code list (`invalid_argument`, `quota_exceeded`, `resource_exhausted`,
  `old_str_not_found`, `old_str_not_unique`, `doc_not_found`, `conflict`,
  `busy`) mapped back to `CoreError`. The trait is
  `#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]`, as
  `ContentStore` is.
- *`AppState`*: set `app_data` inside `assemble` (no new `StateParts`
  field), so the Worker compiles until Task 11 overrides it.
- *Audit*: `doc.write` records live in each `AppData`. The toolpath export
  (`crates/clax-server/tests/api_toolpath.rs`) gathers them from the
  artifacts it exports through `AppDataOp::AuditSince`, so the export
  keeps its document writes; tests that read the instance's `docs` or
  `doc_authors` directly (`golden_single_owner.rs`, `api_visibility.rs`,
  `sessions.rs`, `visibility.rs`, `crates/clax-testkit/src/people.rs`,
  `crates/clax-cli/tests/comments.rs`) are ported to the host.

- [ ] **Step 1: Write the failing tests:**
  `app_data` unit tests ported from today's `docs.rs` tests;
  `the_first_open_moves_an_artifacts_documents_once` (rows in the instance
  → open → rows in `AppData`, none in the instance, second open moves
  nothing, versions keep increasing past the old sequence);
  `the_instance_database_holds_no_documents_after_a_write` (write through
  the route, then `SELECT count(*) FROM docs` on the instance → 0 or no table);
  `deleting_an_artifact_removes_its_data_file`.
- [ ] **Step 2–4:** run (`just test-native -p clax-store app_data`,
  `-p clax-server --test integration api_docs`), implement, run — FAIL then PASS.
- [ ] **Step 5: Commit** `"Keep each artifact's app data in a store of its own"`.

### Task 11: The Worker: site hosts, `AppDataDo`, the site domain

**Files:**
- Modify: `crates/clax-worker/src/lib.rs:118-166` (classify the host first:
  a site host goes to the instance object's site routes and nothing else,
  so `/git/`, `/_diag/`, `/_dev/repos/`, `/do/`, `/mcp` answer 404 there;
  `content_host` removed; `SITE_DOMAIN` var)
- Create: `crates/clax-worker/src/app_data.rs` (`AppDataDo`, a SQLite-backed
  Durable Object over `DoPageStore`, answering `POST /_internal/app-data`
  with an `AppDataOp`; `WorkerAppData: AppDataHost` calling it by
  `get_by_name(artifact ID)`)
- Modify: `crates/clax-worker/src/instance.rs:360-370` (`SITE_DOMAIN` replaces
  `CONTENT_ORIGIN`; refuse a site domain equal to or under the public host)
- Modify: `crates/clax-worker/wrangler.jsonc` (binding `APP_DATA`, class
  `AppDataDo`, migration tag `v4`; vars `SITE_DOMAIN` replaces `CONTENT_ORIGIN`)
- Modify: `crates/clax-worker/e2e-hosted.mjs:209-237, 424-437, 507`
  (site host `<id>.localhost:<port>`, the handshake, world-open, two
  artifacts' storage isolated, the old keyed URL redirect)
- Modify: `.github/workflows/ci.yml:148-149` and `scripts/quality_gates.sh:95`
  (run `e2e-hosted.mjs` beside `e2e.mjs`)
- Modify: `crates/clax-worker/README.md` (vars table, the DNS section)
- Test: the e2e above; `crates/clax-worker` unit tests for the host classification

**Interfaces:**
- Consumes: `AppDataHost`, `AppDataOp`, `AppDataReply` (Task 10); `SiteHosts` (Task 1).

- [ ] **Step 1: Write the failing tests:** unit
  `a_site_host_reaches_no_path_first_route` (each prefix above, on
  `abcdefghjkmn.clax.run` → 404); e2e-hosted steps "site served at the root
  through the handshake", "world-open site served signed out", "two
  artifacts' localStorage and db are separate", "a document lives in the
  artifact's object, not the instance".
- [ ] **Step 2–4:** run (`just worker-test`, `node e2e-hosted.mjs` under
  `crates/clax-worker`), implement, run — FAIL then PASS.
- [ ] **Step 5: Commit** `"Serve sites on the Worker's site domain, with app data in an object per artifact"`.

## Phase D: deploy and docs

### Task 12: `deploy.mjs`, the smoke and the domain

**Files:**
- Modify: `crates/clax-worker/deploy.mjs:180-212, 283-326, 535-562`
  (`--site-domain` replaces `--content`; `--zone example.com` offers
  `clax.run`-style input on a terminal, no default; `hostOf` keeps refusing
  `*`, and a separate `siteDomainOf` validates the domain; the generated
  config gets `routes: [{pattern: pub, custom_domain: true},
  {pattern: "*.<site domain>/*", zone_name: "<site domain>"}]`; the record
  gains `site_domain`; after deploy it checks `https://<random id>.<site domain>/_clax/healthz`
  and, failing, prints the manual steps: the zone on the account, a proxied
  `*` DNS record, a certificate covering `*.<site domain>` (Advanced
  Certificate Manager or Total TLS))
- Modify: `scripts/cloudflare-setup.sh:72-125` (`SITE_DOMAIN` in `setup.env`)
- Modify: `crates/clax-worker/smoke-remote.mjs` (steps: a private site
  refused without the handshake, served after it; a world-open site served
  signed out; two sites' runtime isolated)
- Modify: `docs/usage.md:180-194` (`[hosted] site_domain` for the daemon in
  hosted mode), `crates/clax-worker/README.md`
- Create: `docs/hosted/public-suffix.md` (the PSL submission for clax.run:
  what to file and how to check it shipped)

**Interfaces:**
- Consumes: the Worker's `SITE_DOMAIN` (Task 11).

- [ ] **Step 1: Write the failing test:** `deploy.mjs --dry-run --zone
  example.com --site-domain example.run` writes
  `target/deploy/wrangler.deploy.jsonc` with the wildcard route and
  `SITE_DOMAIN` (a check in `crates/clax-worker/test-deploy-config.mjs`, new,
  run by `just worker-test`).
- [ ] **Step 2–4:** run, implement, run — FAIL then PASS.
- [ ] **Step 5: Commit** `"Deploy the site domain as a wildcard route and smoke a real site host"`.

### Task 13: Close the loop

- [ ] **Step 1:** `qualifier review` and `qualifier diff main`; re-anchor
  drifted records on lines this branch moved.
- [ ] **Step 2:** Resolve ad4d8d8c and 94ab5cda with the passing tests'
  names and `--ref git:<sha>`, in a separate commit; propose closing
  0a1a0d4b, 49249c3c, 18483473 (LAN viewers removed) to the owner, with
  `a_lan_viewer_is_refused` as evidence.
- [ ] **Step 3:** `just check`, logged to `target/check.log` in the background.
- [ ] **Step 4: Commit** the `.qual` changes.

## Threads deliberately not done here

- 808970a3 (a draft page is served without the bridge): drafts keep
  being served unwrapped.
- 2b006cc0, 924f246c (alternatives on keyed drafts and clips): drafts
  keep their token; clips leave keyed addresses for the reader's credential
  (Task 4), which makes 924f246c moot — propose closing it as obsolete.
- ce3cfffe, a7ee35dc (whole files read into memory): unchanged.
- ddaee3f9 (a visitor may become a guest without a link): unchanged.
- Vanity names, custom hostnames, a platform auth API (the spec's non-goals).
