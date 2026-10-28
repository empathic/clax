# Report queues, milestone 1: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A deployer's page files a report with a clip through the
embeddable overlay into a queue on a hosted Clax, with no second sign-in;
an agent lists, claims (lease), replies to and resolves it; the reporter
sees the reply in the overlay. Verified end to end in a browser.

**Architecture:** New store tables for queues, service grants, reporter
tokens, external artifacts and claims; three groups of routes (admin in a
browser session, service and reporter on their own router with their own
auth and CORS, agents through `Agent<N>`); six MCP tools with Pi parity; a
TypeScript `web/sdk` (WebCrypto Ed25519 signer, reporter credential, Node
service helper); an overlay built from `web/bridge`; a shell Queues page.

**Tech stack:** Rust (axum, rusqlite, ed25519-dalek as `crates/clax-core/src/sig.rs`
uses it), TypeScript, Svelte 5, Playwright, Bazel.

**Spec:** `docs/superpowers/specs/2026-10-27-clax-sdk-design.md`

## Scope

Milestone 1 is spec §10 items 3–7, cut to the report path. Not in it,
each a later milestone: the contract generated from the routes (§4.1; M1
writes the new routes' TypeScript types by hand in `web/sdk/src/types.ts`),
the general `/api/external` routes (§6.2; M1 makes external artifacts only
through reports), webhooks (§9), the extension on grants and on hosted
(§5.4), the shell, Pi and CLI on the SDK, and folding `Caller` (§10 items
1, 2, 8, 9).

## Global constraints

- `AGENTS.md` throughout: no `unsafe`; no sleeps or wall-clock budgets in
  tests (the `ManualClock` of `crates/clax-core/src/working.rs`); no hard
  links; "ID" in prose; doc comments state the contract.
- Run `qualifier threads <path>` before editing a file.
- Tests: `just test-affected` only (Bazel picks them); never full suites
  mid-task; never `cargo build|test` or `pnpm install` directly. The
  controller runs `just check` once at the end.
- Commits: `git -c commit.gpgsign=false commit -m …`, your own `-m`; stage
  only your files with a private `GIT_INDEX_FILE`
  (`export GIT_INDEX_FILE=$(git rev-parse --git-dir)/index-<task>; git read-tree HEAD`)
  and check `git diff --cached --stat` before each commit. Several agents
  share this worktree on disjoint files.
- One migration, number 55 (`crates/clax-store/src/migrations.rs`, after
  `ASSET_KEYS_MIGRATION`), with its `Mark` in `MARKS_AFTER_APP` and a
  `QUEUES_MIGRATION` const. Only Task 1 edits it.
- Lease: 10 minutes (`LEASE_SECS = 600`). Reporter token: 15 minutes
  (`REPORTER_TOKEN_SECS = 900`). `external_id` at most 512 bytes; artifact
  metadata at most 64 KiB; thread metadata at most 16 KiB.
- Shared shapes below are the interface between tasks; change one only by
  telling the controller.

## Shared shapes (JSON)

```
Queue   {id, name, origins: [string], reporters_see: "own"|"all", created_at, role: "admin"|"triage"}
Report  {id, queue_id, artifact: {id, external_id, title, url, metadata, metadata_version, revision},
         anchor, status: "open"|"resolved", state: "open"|"claimed"|"resolved",
         reporter_id: string|null, metadata, metadata_version, has_clip,
         comments: [{id, author_kind: "viewer"|"agent"|"reporter", author_name, body, created_at}],
         claim: {holder_name, expires_at}|null, created_at}
Lease   {lease_id, expires_at, fence: number, report: Report}
```

Routes (all JSON unless noted; errors `{error: {code, message}}` as `ApiError` answers them):

| Who | Route |
|---|---|
| admin, browser session (`SameOrigin`) | `POST /api/queues {name, origins, reporters_see}` → `{queue}`; `GET /api/queues` → `{queues}`; `PATCH`/`DELETE /api/queues/{qid}`; `GET /api/queues/{qid}/members` → `{members: [{public_id, name, role}]}`; `GET /api/queues/{qid}/service-grants` → `{grants: [{id, name, created_at}]}`; `PUT /api/queues/{qid}/members/{public_id} {role}`, `DELETE` same; `POST /api/queues/{qid}/service-grants {name}` → `{grant: {id, name, created_at}, token: "cxq_…", private_key: "<PKCS#8, base64url>"}` (shown once); `DELETE /api/queues/{qid}/service-grants/{gid}` |
| service (`Authorization: Clax-Queue <token>` + `Clax-Signature` v2) | `POST /api/queues/{qid}/reporter-tokens {reporter_id, origin, public_key}` → `{token: "cxr_…", expires_at}` |
| reporter (`Authorization: Clax-Reporter <token>` + `Clax-Signature` v2; `Origin` equal to the token's) | `POST /api/queues/{qid}/reporter/reports` (multipart: `external_id?`, `url`, `title`, `anchor`, `body`, `metadata?`, `revision?`, `clip?` PNG) → `{report}`; `GET /api/queues/{qid}/reporter/reports?cursor=` → `{reports, next}`; `POST …/reporter/reports/{tid}/comments {body}` → `{report}`; `GET …/reporter/reports/{tid}/clip`; `OPTIONS` on each |
| triage person or their agent (`Agent<N>`) | `GET /api/queues/{qid}/reports?state=&cursor=` → `{reports, next}`; `GET …/reports/{tid}`; `POST …/reports/{tid}/claim` → `Lease`; `…/renew` → `Lease`; `…/release`; `POST …/reports/{tid}/comments {body}`; `POST …/reports/{tid}/resolve`; `PATCH …/reports/{tid} {metadata, if_version}`; `PATCH …/reports/{tid}/artifact {metadata, if_version}`; `GET …/reports/{tid}/clip` |
| anyone | `GET /.well-known/clax` → `{kind: "local"|"hosted"|"worker", contract_version: "1", sign_in: [...], capabilities: [...]}` |

A report without `external_id` uses the page URL without its route
(`clax_core::live::parse_page_url`'s key, `page_url()`) as its
`external_id` (D17: a URL is a unique identifier).

Errors: `lease_lost` (409, an agent writes to a report whose lease it no
longer holds), `thread_taken` (409, existing, another live claimant),
`external_artifact` (400, content-only routes), `wrong_origin` (403),
`token_expired` (401), `not_a_member` (404, as for an unknown queue).

---

### Task 1: Store, core types and route skeleton (interfaces first)

**Risk:** high (schema). **Files:**
- Modify: `crates/clax-store/src/migrations.rs` (migration 55), `crates/clax-store/src/lib.rs` (mod lines), `crates/clax-core/src/audit/mod.rs` (kinds `external.create`, `external.update`, `thread.metadata`, `queue.create`, `queue.update`, `queue.claim`, `queue.release`), `crates/clax-core/src/lib.rs`
- Create: `crates/clax-core/src/queue.rs` (types, limits, `LEASE_SECS`, `REPORTER_TOKEN_SECS`, `Report`, `Queue`, `Lease`), `crates/clax-store/src/queues.rs`, `crates/clax-store/src/external.rs`
- Create (skeleton, handlers answering 501 `not_yet`): `crates/clax-server/src/routes/queues.rs` (admin), `crates/clax-server/src/routes/queue_agent.rs` (triage), `crates/clax-server/src/routes/queue_reporter.rs` (service + reporter, with `pub fn router(state) -> Router`), `crates/clax-server/src/routes/discovery.rs`; register them in `router.rs`, `routes/mod.rs`, and the admin and triage routes' needs in `crates/clax-http/src/agent.rs` (`ROUTES`, `ROUTE_NEEDS`, new `need!`s on `Artifacts`/`Comments` with class `Own`). The reporter router is merged outside the `authenticate`/`require_credential`/`hosted::gate` layers, as `/healthz` is (router.rs:659).

**Tables (migration 55):** `queues(id, name, person, origins_json, reporters_see CHECK own|all, created_at, deleted_at)`; `queue_members(queue_id, viewer_id, role CHECK admin|triage, PK(queue_id, viewer_id))`; `queue_service_grants(id, queue_id, name, token_sha256 UNIQUE, public_key, created_at, revoked_at)`; `queue_reporter_tokens(token_sha256 PK, queue_id, service_grant_id, reporter_id, origin, public_key, expires_at)`; `external_artifacts(artifact_id PK REFERENCES artifacts, external_id UNIQUE, url, metadata_json, metadata_version, revision)`; `queue_claims(thread_id PK, queue_id, session_id, lease_id, fence, expires_at)`; `threads` gains `queue_id`, `reporter_id`, `metadata_json`, `metadata_version`. External artifacts keep `artifacts.kind = 'html'` with a placeholder version 1 linking to `url` (`placeholder_html` in `crates/clax-core/src/live.rs` as a model); `Artifact` serializes `kind: "external"` when an `external_artifacts` row exists.

**Store API (produces):**
- `create_queue(ctx, person: Option<&str>, NewQueue{name, origins, reporters_see}) -> Result<Queue>`, `queues_of(person) -> Vec<Queue>`, `queue(qid) -> Option<QueueRow>`, `update_queue`, `delete_queue`, `set_member(ctx, qid, viewer_id, Option<Role>)`, `role_of(qid, viewer: Option<&str>) -> Option<Role>` (the queue's creator is `admin`).
- `create_service_grant(ctx, qid, name) -> Result<(ServiceGrant, Secret /*token*/, KeyPair)>` (key generated with `clax_core::sig::KeyPair`), `service_grant_by_token(sha256) -> Option<ServiceGrant>`, `revoke_service_grant`.
- `mint_reporter_token(ctx, &ServiceGrant, reporter_id, origin, public_key) -> Result<(Secret, String /*expires_at*/)>` (refuses an origin not in the queue's `origins`: `wrong_origin`), `reporter_by_token(sha256) -> Option<ReporterToken>` (expired → `None`).
- `file_report(ctx, qid, reporter: Option<&str>, NewReport{external_id: Option, url, title, anchor, body, author_name, metadata, revision, clip}) -> Result<Report>`: finds or creates the external artifact (`external.create`), creates the thread through `create_thread_then_async` setting `queue_id`, `reporter_id`, `metadata_json` in the same transaction.
- `reports(qid, Visibility::{All, Reporter(&str)}, state: Option<State>, cursor) -> (Vec<Report>, Option<String>)`, `report(qid, tid, Visibility) -> Option<Report>`.
- `claim(ctx, qid, tid, session_id) -> Result<Lease>` (sets `threads.target_session_id` too; 409 `thread_taken` when another session holds a live lease), `renew(ctx, qid, tid, session_id) -> Result<Lease>`, `release(ctx, qid, tid, session_id)`, `renew_session_leases(session_id, thread_ids: &[String])`, `check_lease(qid, tid, session_id) -> Result<()>` (`lease_lost` when the session held an expired or superseded lease, `thread_taken` when another holds it, Ok when it holds it). Expiry by `st.now()`; an expired lease clears `target_session_id`.
- `set_thread_metadata(ctx, qid, tid, value, if_version) -> Result<u64>` (`thread.metadata`, body per spec §6.6), `set_artifact_metadata(ctx, aid, value, if_version) -> Result<u64>` (`external.update`).
- `reporter_comment(ctx, qid, tid, reporter_id, body)` (author_kind `reporter`, author_name the reporter ID).

- [ ] Step 1: tests in `queues.rs`/`external.rs` (`store_with_clock(ManualClock)`): `claim_then_expiry_frees_report` (advance 601 s → another session claims; first gets `lease_lost` from `check_lease`), `renew_extends_lease`, `reporter_sees_only_own_under_own`, `same_external_id_two_queues_one_artifact_threads_apart`, `metadata_if_version_conflict`, `metadata_write_records_audit_with_value_and_hashes`, `reporter_token_expires_after_900s`, `mint_refuses_origin_not_in_queue`. Run `just test-affected`: they fail.
- [ ] Step 2: implement; `just test-affected` passes. Commit "Add report queues, external artifacts and leases to the store".
- [ ] Step 3: route skeleton compiles, guard tests pass (`crates/clax-server/src/routes/guards.rs`; regenerate the route table in `docs/contract.md` with `CLAX_UPDATE_GOLDEN=1` through the Bazel test's documented way). Commit "Register the queue routes".

### Task 2: Admin routes and discovery

**Risk:** medium. **Files:** `routes/queues.rs`, `routes/discovery.rs`, tests `crates/clax-server/tests/api_queues.rs`. **Consumes:** Task 1's store API. Admin routes take `SameOrigin` and a browser person (or the owner); a non-member gets 404 `not_a_member`.
- [ ] Tests: create/list/patch/delete a queue; members; a service grant's token and key shown once and its revoke ending reporter tokens it minted; another person's queue 404; discovery shape on local and hosted (`TestServer::spawn_hosted`).
- [ ] Implement; `just test-affected`; commit.

### Task 3: Service and reporter auth and routes

**Risk:** high (auth). **Files:** create `crates/clax-http/src/reporter.rs` (extractors `ServiceCaller`, `ReporterCaller`; verify with `clax_core::sig::verify` over `grant_auth::input`-shaped lines with the token's own key; nonce and skew as `agent::check_head`), `routes/queue_reporter.rs`, tests `crates/clax-server/tests/api_queue_reporter.rs`. CORS: answer `OPTIONS` and set `Access-Control-Allow-Origin` to the request's `Origin` only when it is in the queue's `origins` (never `*`); allow headers `authorization, clax-signature, content-type`.
- [ ] Tests: mint with a good and a bad signature; reporter files a report with a clip (multipart) and reads it back; `Origin` other than the token's → 403 `wrong_origin`; no `Origin` → 403; expired token (ManualClock +901 s) → 401 `token_expired`; reporter under `own` cannot read another reporter's report (404); preflight answers only listed origins; a reporter token on every non-reporter route → 401.
- [ ] Implement; `just test-affected`; commit.

### Task 4: Triage routes, MCP tools and Pi parity

**Risk:** high. **Files:** `routes/queue_agent.rs`, tests `crates/clax-server/tests/api_queue_agent.rs`; `crates/clax-mcp/src/tools.rs`, `client.rs`, `render.rs`; `plugins/pi/src/clax.ts`, `plugins/pi/test/fixtures/contract.json`, `scripts/test-plugins.sh` (count comment); `docs/contract.md` ("Tools", tool table); `working`'s route in `crates/clax-server/src/routes/working.rs` calls `renew_session_leases` for its `thread_ids`. Replies and resolves by an agent call `check_lease` first.
- [ ] Tools (38 in all): `queue_list {queue?, state?, cursor?}` (no `queue`: the caller's queues), `queue_claim {queue, report_id}`, `queue_renew {queue, report_id}`, `queue_release {queue, report_id}`, `queue_reply {queue, report_id, body}`, `queue_resolve {queue, report_id}`. Each result is the route's JSON.
- [ ] Tests: two agent sessions race a claim (one wins, other 409); expiry then reclaim, old holder's reply 409 `lease_lost`; `working` renews; non-member 404; metadata PATCH with stale `if_version` 409; `tool_count_is_38`, `tools_match_the_fixture`; Pi's tool set test.
- [ ] Implement; `just test-affected`; commit.

### Task 5: `web/sdk`

**Risk:** medium. **Files:** create `web/sdk/src/{types.ts, sign.ts, client.ts, reporter.ts, service.ts}` with tests beside them; `web/BUILD.bazel` and `web/tsconfig.json` to include `sdk/`. `sign.ts` implements `clax-ed25519 v2` (the lines of `crates/clax-core/src/sig.rs` `SignInput`; `plugins/pi/src/remote.ts` `canonical` is the reference) with WebCrypto Ed25519 (`crypto.subtle`, non-extractable key, `generateKey({name: "Ed25519"}, false, ["sign"])`); `service.ts` (Node) signs with the PKCS#8 key from a service grant and calls `reporter-tokens`.
- [ ] `ReporterClient({instance, queue, auth: (publicKeyB64: string) => Promise<{token, expires_at}>})` with `fileReport`, `myReports`, `comment`, `clipUrl`; renews the token 60 s before `expires_at`.
- [ ] Tests: the canonical lines equal a vector produced by the Rust signer (add one fixed vector test in `crates/clax-core/src/sig.rs` and copy its bytes); renewal timing with an injected clock; 401 triggers one renewal and one retry.
- [ ] `just test-affected`; commit.

### Task 6: The embeddable overlay

**Risk:** medium (UI; verify in a browser). **Files:** create `web/overlay/src/{main.ts, Overlay.svelte, Reports.svelte}`, `web/vite.overlay.config.ts` (IIFE `dist/_clax/overlay.js`, global `Clax`), add to `package.json` `build`; reuse `bridge/src/comment-mode.ts`, `anchor.ts`, `area.ts`, `clip.ts` (`renderTargetClip`, `renderAreaClip`) as `web/extension/src/content/overlay-app.ts` does. `Clax.overlay({instance, queue, auth, metadata?, externalId?})`; `externalId` defaults to `<meta name="clax-artifact">`, then the SDK leaves it out (the server uses the URL). A launcher button, comment mode, a composer (text + clip preview + send), and "Your reports" with replies, polling every 15 s while open.
- [ ] Unit tests for the composer model; `just test-affected`; commit. Browser check in Task 8.

### Task 7: Shell Queues page

**Risk:** low (UI). **Files:** `web/shell/src/queues/Queues.svelte` (+ model and test), `web/shell/src/ui/gallery-working.ts` (`PAGES`), `web/shell/src/ui/Gallery.svelte` path regex, `crates/clax-server/src/router.rs` `/queues` → `shell::gallery_page` (Task 1 adds this line), `crates/clax-http/src/hosted.rs` guest mapping, `scripts/bundle-size.mjs` budget. Create a queue (name, origins one per line, reporters see own/all), members by public ID with role, service grants (create shows token and key once with copy buttons and the warning that they are not shown again; revoke), and each queue's open/claimed/resolved counts.
- [ ] Model tests; `just test-affected`; commit. Browser check in Task 8.

### Task 8: End to end in a browser

**Risk:** medium. **Files:** `web/e2e/queues.spec.ts`, `web/e2e/deployer-site/` (a page loading `/_clax/overlay.js` and a tiny Node backend using `web/sdk/src/service.ts`), project entry in `web/playwright.config.ts` and a Bazel target `//web:e2e_queues`.
- [ ] Spec: `hostedDaemon`; the owner creates a queue on the Queues page with the deployer site's origin and a service grant (read key and token from the page); the deployer backend mints tokens; on the deployer page a reporter opens the overlay, picks an element, types, sends; the report has a clip (`has_clip`, clip GET returns a PNG); an agent of the owner, through the triage routes with an agent token (as the hosted specs' agents call the API), claims and replies (the MCP tools over the same routes are tested in Task 4); the overlay's "Your reports" shows the reply; a second reporter under `own` does not see it; screenshots of the overlay and the Queues page saved under `target/` for the controller to look at.
- [ ] `bazel test //web:e2e_queues`; commit.

### Task 9: Whole-branch review, fixes, gates

Controller: 3 reviewers at once (store and auth; triage, MCP and Pi; web: SDK, overlay, shell), then one fix pass for Critical and Important findings, the rest recorded in qualifier; `just check` once; `qualifier diff main`.

## Order and parallelism

Task 1 first (interfaces). Then Tasks 2, 3, 4 and 5 at once (disjoint files: 2 owns `queues.rs`/`discovery.rs`, 3 owns `queue_reporter.rs`/`reporter.rs`, 4 owns `queue_agent.rs`, `working.rs`, MCP and Pi, 5 owns `web/sdk`). Tasks 6 and 7 start when 5 has committed `types.ts` and `sign.ts`. Task 8 after 3, 4, 6, 7. At most two Bazel builds at once; disk watched (24 GiB free at start).

## Review focus

- A reporter token replayed from another origin, or with `Origin` stripped (server-side fetch): refused (Task 3 tests).
- A lease that expires mid-reply: the late write is refused, never applied to a report another agent holds (Task 1, Task 4 tests).
- Two apps sending one `external_id`: one artifact, threads kept to their queues (Task 1 test).
- A clip the DOM renderer cannot draw (cross-origin image): the report still files, with a partial clip (Task 6: `renderTargetClip` already reports `clipError`; the composer sends without a clip on error).
- An overlay on a page with a strict CSP or many iframes: comment mode still starts in the top document (Task 8 spec runs the deployer page with a CSP header that allows only the instance origin for `connect-src`).
