# Apps on clax.dev: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the Clax app publish its apps to a hosted Clax, share them with
the hosted Clax's roles, and get, update and co-author them from any device.
Comments flow back to the app.

**Architecture:**
- **Hosted side.** The hosted Clax (the daemon in hosted mode and the
  Worker; both run `clax-server`) gains:
  - signed-bundle storage per version, with a head route;
  - a per-artifact comments switch;
  - a "person's device" token marker that admits the Clax app to the
    sharing routes;
  - a server-rendered `/a/<id>/get` page.
- **App's embedded core.** It gains a single-version push mode and proxy
  routes to the remote (head, bundle, files, sharing, apps). The app
  never holds the remote token.
- **Dart side.** It adds:
  - a pure sync-state function;
  - a `linked` trust answer;
  - flows (connect, publish, share, get, update, merge, diverge);
  - a foreground update checker on an injected clock.

**Tech Stack:** Rust (axum, rusqlite, clax-* crates, Worker on wasm32);
Svelte 5 shell (vitest, Playwright); Flutter/Dart app; Bazel for every build
and test.

**Spec:** `docs/superpowers/specs/2026-10-21-app-push-pull-design.md` (this
branch). Every task cites the spec sections it implements. Read only those.

## Global Constraints

- **Builds and tests.** Bazel only. Run `just test-affected` while working.
  Never run `cargo`, `pnpm install` or `flutter` directly in the worktree
  except through `-native` recipes (AGENTS.md).
- **No `unsafe`.** No hard links.
- **Tests:** no sleeps, no wall-clock budgets, injected clocks. Every test
  must be able to fail: break the guarded code in a scratch copy and watch
  it fail, but never commit that copy.
- **Fakes** refuse what the real thing refuses. An unrouted fake call fails
  the test loudly.
- **Migration:** exactly one new migration, **48**, appended after 47 in
  `crates/clax-store/src/migrations.rs`. Every task that needs schema adds
  to migration 48 in place. Update the length and "reaches" tests
  (`the_app_migrations_follow_mains_in_this_order`, `a_fresh_database_reaches_47`
  → 48). It needs no adoption step: no installed build has run 48.
- **Crate layering** per `scripts/check-crate-layers.py`. After any
  `Cargo.toml` change, run `just bazel-sync` and commit the result.
- **App copy:** curly apostrophes and quotes, "…", "Claude" for the agent.
  `//scripts:check_copy` covers `lib/ui/**`, `lib/home/**`, `lib/app.dart`
  and `home/*.js`.
- **App UI isolate only draws.** Core calls go through `ClaxApi` (the
  scheme worker isolate), and byte work through `Isolate.run` or
  `platform/offload.dart`.
- **Web bundle budgets** (`web/perf/bundle-budget.json`): new shell UI is
  lazy. Raise a budget by its real cost if needed; never contort code to
  fit.
- **"ID" in prose.** Doc comments and commit messages describe the
  contract or change, never this plan or a conversation.
- **Commits:** commit each finished task with
  `git -c commit.gpgsign=false commit -m "…"`. Check
  `git diff --cached --stat` first. Never push.
- **Spec values pinned:**
  - bundle cap 32 MiB (33,554,432 bytes);
  - update check at most once per 15 minutes per remote;
  - comments off and general access Restricted on an app's first publish;
  - Include my data off by default and never remembered;
  - MIME `application/vnd.empathic.clax+zip`.

## Review Focus

1. **Lost answer during publish.** The push succeeded on the remote but its
   answer was lost. The retry must not make a second public version, and
   the bundle upload retries until it lands. Owned by Task 4; test
   `app_push_replays_after_lost_answer`.
2. **Same old version published twice.** Publishing local v3, then v5, then
   v3 again must make three public versions, not a replay. Owned by Task
   4; test `app_push_same_local_version_twice_makes_two_public_versions`.
3. **Bundle and version disagree.** A recipient whose downloaded bundle's
   `content_sha256` differs from the head's must refuse it with nothing
   written. Owned by Task 9; test
   `pull_refuses_bundle_whose_hash_differs_from_head`.
4. **Comments off.** With comments off, an owner's own comment, a
   commenter's comment, a guest's comment, a reply and a linked thread
   mirrored up are all refused with `comments_off`, and the app hides the
   composer. Owned by Tasks 2 and 13; tests named there.
5. **Disconnected or offline with a stale head.** A linked app whose remote
   was removed or is unreachable keeps working. The checker records no
   failure state that blocks launch, and it never shows ↓ from a cached
   head older than what the device holds. Owned by Task 10; test
   `checker_offline_keeps_state_and_never_regresses`.

---

## Tracks and order

| Track | Tasks | Depends on |
|---|---|---|
| R (Rust, hosted + core) | 1 → (2, 3, 5 in parallel) → 4 | 1's interfaces |
| W (web shell) | 6 | 2's JSON fields |
| D (Dart app) | 7 → (8, 9, 10, 11, 12, 13 by file sets) → 14 | 4's core routes; 7's interfaces |
| Docs and end to end | 15, 16 | all |

Task 1 and Task 7 commit their interfaces first, in their own commits. Later
tasks build against them in parallel worktrees. File sets are disjoint
within a parallel group; the controller serializes any overlap.

---

### Task 1: Store schema and APIs (migration 48)

Spec §5.6, §5.8, §5.9. Risk: high (schema).

**Files:**
- Modify: `crates/clax-store/src/migrations.rs` (append 48; update tests at ~4218, ~4248)
- Create: `crates/clax-store/src/app_bundles.rs`
- Modify: `crates/clax-store/src/lib.rs` (module), `crates/clax-store/src/artifacts.rs` (comments column read/write), `crates/clax-store/src/device.rs` (client kind), `crates/clax-store/src/hosted.rs` (device grant marker), `crates/clax-store/src/remotes.rs` (link without push)
- Test: unit tests in each file; `crates/clax-store/tests/app_bundles.rs`

**Interfaces (Produces):**
- **Migration 48:**
  - `CREATE TABLE app_bundles (artifact_id TEXT NOT NULL, version INTEGER NOT NULL, lineage TEXT NOT NULL, bundle_version INTEGER NOT NULL, content_sha256 TEXT NOT NULL, sha256 TEXT NOT NULL, size INTEGER NOT NULL, person TEXT, created_at TEXT NOT NULL, PRIMARY KEY (artifact_id, version))`
  - `ALTER TABLE artifacts ADD COLUMN comments INTEGER NOT NULL DEFAULT 1`
  - `ALTER TABLE device_codes ADD COLUMN client_kind TEXT`
  - `ALTER TABLE grants ADD COLUMN person_device INTEGER NOT NULL DEFAULT 0`
  - `CREATE TABLE app_installs (artifact_id TEXT NOT NULL, install_sha256 TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY (artifact_id, install_sha256))`
- `pub const MAX_BUNDLE_BYTES: u64 = 33_554_432;`
- `pub struct AppBundle { artifact_id, version: u32, lineage: String, bundle_version: u64, content_sha256: String, sha256: String, size: u64, person: Option<String>, created_at: String }`
- `Store::put_app_bundle_async(&self, ctx: &AuditCtx, aid: &ArtifactId, version: u32, lineage: &str, bundle_version: u64, content_sha256: &str, bytes: &[u8], person: Option<&str>) -> Result<AppBundle>`. It refuses with `CoreError::invalid(code, …)`:
  - `bundle_too_large`: over the cap;
  - `bundle_mismatch`: the version's `content_sha256` differs;
  - `bundle_lineage`: the lineage differs from the artifact's first bundle;
  - `bundle_version_order`: `bundle_version` ≤ the artifact's max.

  Bytes go to the content store through `put_asset` with an `AssetKey` of
  ext `clax`. Re-putting identical bytes for the same `(aid, version)` is a
  no-op that returns the row.
- `Store::app_bundle(&self, aid, version) -> Result<Option<AppBundle>>`, `Store::latest_app_bundle(&self, aid) -> Result<Option<AppBundle>>`, `Store::read_app_bundle_async(&self, b: &AppBundle) -> Result<Vec<u8>>`
- `Store::set_comments(&self, ctx, aid, on: bool) -> Result<()>`, `Store::comments_on(&self, aid) -> Result<bool>`
- `Store::record_install(&self, aid, install_sha256: &str) -> Result<()>` (idempotent), `Store::install_count(&self, aid) -> Result<u64>`, `Store::fork_count(&self, aid) -> Result<u64>` (artifacts whose `forked_from` is `aid`)
- `Store::start_device_kind(&self, client_name, peer, kind: Option<&str>)`: `start_device` gains `kind`. Approval sets `grants.person_device = 1` when the kind is `"clax-app"`. Also `Store::is_person_device_grant(&self, grant_id) -> Result<bool>`.
- `Store::link_remote(&self, aid, rid, remote_aid, url, local_n: u32, remote_n: u32) -> Result<RemoteLink>`: creates or updates a link without a push (`pushed_n = local_n`, `auto = false`).

- [ ] **Step 1:** Write failing tests:
  - `migration_48_adds_bundles_comments_device_and_installs`;
  - `put_app_bundle_refuses_mismatch_lineage_order_and_size`, with each code asserted;
  - `put_app_bundle_same_bytes_twice_is_one_row`;
  - `comments_default_on_and_toggle`;
  - `device_kind_clax_app_marks_grant`;
  - `link_remote_without_push_sets_base`;
  - `install_count_counts_distinct_installs`;
  - `fork_count_counts_forks`.
- [ ] **Step 2:** Run `just test-affected` → FAIL.
- [ ] **Step 3:** Commit the interfaces (signatures plus `todo!()`-free stubs returning `Err(CoreError::invalid("unimplemented",…))`, and migration 48) as `Add store interfaces for app bundles, the comments switch and device grants`.
- [ ] **Step 4:** Implement.
- [ ] **Step 5:** `just test-affected` → PASS. Commit.

---

### Task 2: Hosted routes: bundles, head, comments switch, apps library, installs

Spec §5.3 (3), §5.5, §5.6 (Comments), §5.7 (head), §5.9. Risk: high.

**Files:**
- Create: `crates/clax-server/src/routes/app_bundles.rs`
- Modify: `crates/clax-server/src/router.rs` (mount), `crates/clax-http/src/agent.rs` (`need!` entries), `crates/clax-http/src/hosted.rs` (`guest_need` for GET head and bundle; POST installs as anonymous), `crates/clax-http/src/limits.rs` (`BUNDLE_BODY_LIMIT = 33_554_432 + 64 KiB`), `crates/clax-server/src/routes/threads.rs` (refuse when off), `crates/clax-server/src/routes/artifacts.rs` (artifact JSON fields; `comments: false` on create), `crates/clax-server/src/shell/boot.rs` (same fields), `crates/clax-server/src/hosted.rs` (comments switch route; `comments` in `get_sharing`; apps library), `crates/clax-server/tests/golden_single_owner.rs` (new keys)
- Test: `crates/clax-server/tests/api_app_bundles.rs`

**Interfaces (Produces):**
- **`PUT /api/artifacts/{aid}/versions/{n}/bundle`**
  - Needs `Artifacts update`.
  - Raw body, content type `application/vnd.empathic.clax+zip`.
  - Headers `clax-bundle-lineage`, `clax-bundle-version`, `clax-bundle-content-sha256`.
  - Answers 201 `{artifact_id, version, bundle_version, sha256, size}`.
  - Errors: 400 with the store codes, 404 for no version, 413 `bundle_too_large`.
- **`GET /api/artifacts/{aid}/versions/{n}/bundle`**
  - Needs `Artifacts read`; guests and link viewers allowed.
  - Returns the bytes with that MIME and `content-disposition: attachment; filename="<slug>.clax"`. 404 when none.
- **`GET /api/artifacts/{aid}/head`**
  - Same access as reading the artifact.
  - Answers `{artifact_id, url, title, version, content_sha256, published_by: {name, login|null}, owner: {name, login|null}, comments: bool, bundle: null | {version, lineage, bundle_version, content_sha256, url, published_by: {name, login|null}}}`.
  - `bundle` is the latest version that carries one. `url` is the GET bundle route.
- **`POST /api/artifacts/{aid}/installs {install: "<64 hex>"}`**
  - Allowed anonymously on artifacts the caller may read; rate-limited per client address like guest names.
  - Answers 204.
- **`PUT /api/hosted/artifacts/{aid}/comments {on: bool}`**
  - Same rights as `put_share` (owner or maker).
  - Accepts a browser session or a person-device token (Task 3 wires the token).
- **Comments enforcement:** `POST /api/artifacts/{aid}/threads` and `POST …/threads/{tid}/comments` refuse 403 `comments_off` when the switch is off, for every caller including the owner.
- **`POST /api/artifacts`** body gains optional `comments: bool` (default true).
- **Artifact JSON** (`GET /api/artifacts/{aid}` and the shell bootstrap) gains `comments: bool`, `forks: u64` and `app: null | {bundle_version, installs, published_by:{name,login|null}, owner:{name,login|null}}`. `app` is non-null when the artifact has a bundle.
- **`GET /api/hosted/apps`**
  - For the signed-in person (browser or token): the artifacts with at least one bundle that they own or hold a share on.
  - Answers `[{artifact_id, title, url, version, bundle_version, access: "owner"|"editor"|"commenter"|"viewer", general: "restricted"|"link"|"public"}]`.

- [ ] **Step 1:** Write failing tests in `api_app_bundles.rs`, using `clax_testkit::remotes::Public` and `TestServer::spawn_hosted`:
  - `bundle_put_get_roundtrip_and_head`;
  - `bundle_put_refuses_hash_mismatch` (asserts `bundle_mismatch`);
  - `bundle_put_editor_allowed_viewer_refused`;
  - `head_link_guest_reads_restricted_guest_404`;
  - `comments_off_refuses_owner_commenter_guest_and_reply` (Review Focus 4);
  - `create_with_comments_false`;
  - `installs_count_distinct`;
  - `apps_library_lists_owned_and_shared_with_access`;
  - `artifact_json_carries_app_comments_forks`.
- [ ] **Step 2:** `just test-affected` → FAIL.
- [ ] **Step 3:** Implement. The remote verifies no zip and no signature (spec §5.3 step 3).
- [ ] **Step 4:** `just test-affected` → PASS. Also run `just worker-test` (the Worker shares these routes). Commit.

---

### Task 3: Person-device tokens on the sharing routes

Spec §5.2, §5.6. Risk: high (auth).

**Files:**
- Modify: `crates/clax-server/src/hosted.rs` (`DeviceCodeBody` gains `client_kind: Option<String>`, accepting only `"clax-app"`; `browser_only` becomes `browser_or_device(headers, grant)` on the sharing and comments-switch routes only; `member_of` resolves a person-device grant to its person), `crates/clax-http/src/agent.rs` (`verify_bearer` admits person-device grants on exactly the sharing, comments-switch and `GET /api/hosted/apps` routes), `crates/clax-http/src/hosted.rs` (gate passes those requests), `crates/clax-server/src/public.rs` (`start_sign_in(origin, name, kind: Option<&str>)`), `crates/clax-server/src/routes/remotes.rs` (`AddBody.kind: Option<String>`)
- Test: `crates/clax-server/tests/api_hosted.rs` (new tests near the device sign-in tests ~3445)

**Interfaces:**
- **Consumes:** `Store::is_person_device_grant` (Task 1).
- **Produces:**
  - `POST /api/remotes {url, name?, machine?, kind?: "clax-app"}`.
  - People on the remote lists person-device tokens under "Devices", apart from "Agents". This uses the existing token list JSON with `kind: "device"|"agent"`.

- [ ] **Step 1:** Failing tests:
  - `device_kind_token_manages_sharing_of_own_artifact`;
  - `device_kind_token_refused_on_others_artifact` (`not_permitted`);
  - `plain_agent_token_still_refused_on_sharing` (`grants_refused`);
  - `device_kind_token_refused_on_people_and_approvals`;
  - `device_kind_token_lists_apps`.
- [ ] **Step 2:** FAIL → implement → PASS (`just test-affected`, `just worker-test`).
- [ ] **Step 3:** Commit.

---

### Task 4: The core's app mode: single-version push, bundle forwarding, remote proxies

Spec §5.3, §5.5 (link record), §5.7 (how it checks), §5.8, §5.10. Risk: high.

**Files:**
- Modify: `crates/clax-server/src/public.rs` (`Mode::App { version: u32 }`; `push` sends exactly that local version as the next remote version, with `files_of(v, rewrite=false)`; a `/_blob/` reference refuses 400 `uses_assets`; the create body carries `comments: false`; idempotency key `clax-push:{aid}:{n}:{target}:on:{remote_n}`; moved check before sending; `Client` methods for the proxies below)
- Modify: `crates/clax-server/src/routes/remotes.rs` (`PushBody.version: Option<u32>`, required when `mode == "app"`; new routes below)
- Modify: `crates/clax-server/src/router.rs`
- Test: `crates/clax-server/tests/api_remotes_app.rs`

**Interfaces:**
- **Consumes:** Task 1 (`link_remote`), Task 2 routes, Task 3 `kind`.
- **Produces** (local core routes; owner only, refused on hosted with `local_only`):
  - `POST /api/artifacts/{aid}/push {remote?, mode: "app", version: N}` → `{status:"pushed", url, remote, link, pushed:[{n, remote_n}]}`, or 409 `remote_moved` (extras unchanged), or 400 `uses_assets`.
  - `PUT /api/artifacts/{aid}/remotes/{rid}/bundle?remote_n=M`: raw bundle body plus the three `clax-bundle-*` headers. The core forwards it to the remote's PUT bundle route with the token and passes refusals through. The idempotent replay of identical bytes answers 201.
  - `POST /api/remotes/{rid}/links {artifact_id, remote_artifact_id, url, remote_n}` → the link (`Store::link_remote` with `local_n` = the local current version).
  - `GET /api/remotes/{rid}/artifacts/{raid}/head` → the remote head JSON as is.
  - `GET /api/remotes/{rid}/artifacts/{raid}/versions/{n}/bundle` → bytes, capped at `MAX_BUNDLE_BYTES` (`Client::get_bytes`).
  - `GET /api/remotes/{rid}/artifacts/{raid}/versions/{n}/files` and `…/files/{*path}` → the remote's list and raw file, for taking a web-made head.
  - `GET|PUT|DELETE /api/remotes/{rid}/artifacts/{raid}/sharing[/link|/shares|/shares/{sid}|/comments]` → forwards to `/api/hosted/artifacts/{raid}/…`.
  - `GET /api/remotes/{rid}/apps` → forwards `GET /api/hosted/apps`.
  - `DELETE /api/remotes/{rid}/artifacts/{raid}` → deletes the public copy (Unpublish), then unlinks locally.

- [ ] **Step 1:** Failing tests, using `Public::new` plus `connect`:
  - `app_push_sends_only_the_chosen_version`;
  - `app_push_same_local_version_twice_makes_two_public_versions` (Review Focus 2);
  - `app_push_replays_after_lost_answer` (Review Focus 1, using the existing lost-answer harness at `api_remotes.rs:538`);
  - `app_push_refuses_blob_refs`;
  - `app_push_creates_with_comments_off_and_restricted`;
  - `app_push_after_web_edit_is_remote_moved`;
  - `bundle_forward_roundtrip`;
  - `link_without_push_then_app_push_bases_on_it`;
  - `proxies_forward_head_bundle_files_sharing_apps`;
  - `editor_pull_then_push_publishes_as_editor` (the head's `published_by` is the editor);
  - `unpublish_deletes_remote_and_unlinks`.
- [ ] **Step 2:** FAIL → implement → PASS (`just test-affected`).
- [ ] **Step 3:** Commit.

---

### Task 5: The `/a/<id>/get` page, app links and QR

Spec §5.5. Risk: medium.

**Files:**
- Create: `crates/clax-server/src/shell/get_page.rs`
- Modify: `crates/clax-server/src/router.rs` (explicit `/a/{aid}/get` route before the catch-all; `/.well-known/apple-app-site-association` and `/.well-known/assetlinks.json` when configured), `crates/clax-server/src/shell/route.rs` and `web/shell/src/route.ts` plus `route-cases.json` (`get` is reserved: a page file literally named `get` is served at `/a/<id>/get/` only through the version path), `crates/clax-core/src/config.rs` or the `[hosted]` table (`app_links = {apple_team_id, android_package, android_sha256}` optional; `stores = {ios, android}` optional URLs), `Cargo.toml` plus `crates/clax-server/Cargo.toml` (`qrcode = { version = "0.14", default-features = false, features = ["svg"] }`), then `just bazel-sync`
- Test: `crates/clax-server/tests/get_page.rs`

**Interfaces (Produces):**
- **`GET /a/{aid}/get`:** a self-contained HTML page in the style of `github.rs::failed` (inline CSS, light/dark tokens, legal footer, no JS). Access follows `guest_need` for reading the artifact. It shows:
  - the title and icon word;
  - "v<N> · by @owner";
  - a QR code (inline SVG) of `https://<origin>/a/<id>/get`;
  - **Open in Clax for Mac** → `clax:get?url=<urlencoded https://<origin>/a/<id>>`;
  - **Download .clax** → the latest bundle URL;
  - store links when configured.

  With no bundle it answers 404 with "This page isn’t an app yet.".
- **AASA:** applinks `{"applinks":{"details":[{"appIDs":["<team>.dev.empathic.clax"],"components":[{"/":"/a/*/get"}]}]}}`.
- **assetlinks:** `[{"relation":["delegate_permission/common.handle_all_urls"],"target":{"namespace":"android_app","package_name":"<pkg>","sha256_cert_fingerprints":["<fp>"]}}]`.

- [ ] **Step 1:** Failing tests:
  - `get_page_has_qr_open_and_download`;
  - `get_page_404_without_bundle`;
  - `get_page_restricted_needs_sign_in`;
  - `well_known_served_only_when_configured`;
  - `route_cases_reserve_get` (route-cases.json).
- [ ] **Step 2:** FAIL → implement → PASS (`just test-affected`, `just web-test`).
- [ ] **Step 3:** Commit.

---

### Task 6: Web shell: comments switch, gating and the app header

Spec §5.5 (header), §5.6, §5.11, §5.12. Risk: medium.

**Files:**
- Modify: `web/shell/src/hosted/api.ts` (`Sharing.comments: boolean`; `setComments(aid, on): Promise<void>` → `PUT /api/hosted/artifacts/<aid>/comments`), `web/shell/src/ui/share-model.ts` (`COMMENTS_LABEL = "Comments"`, `COMMENTS_NOTE = {on: "People who can comment may comment.", off: "Off: nobody can comment, whatever their access."}`), `web/shell/src/ui/ShareDialog.svelte` (switch above "People with access", through `act`), `web/shell/src/api.ts` (`Artifact.comments?: boolean`, `forks?: number`, `app?: {bundle_version:number, installs:number, published_by:{name:string,login:string|null}, owner:{name:string,login:string|null}} | null`), `web/shell/src/ui/TopbarIsland.svelte` plus `view/artifact-controller.ts` plus `ui/ShellSidebar.svelte` plus `caps/comments.ts` (every spectator gate also checks `comments === false`; the button title is "Comments are off for this page"), `web/shell/src/view/publisher.ts` (`appByline(app, version): string` → "v8 by @junp · by @akesling" when they differ, else "v8 · by @akesling"; `forksText(n)` → "1 fork"/"N forks")
- Create: `web/shell/src/ui/GetInClax.svelte` (lazy; a link button to `/a/<id>/get`, shown when `artifact.app` is non-null)
- Test: `web/shell/src/share.test.ts`, `web/shell/src/topbar.test.ts`, `web/shell/src/view/publisher.test.ts`, `web/e2e/sharing.spec.ts`

- [ ] **Step 1:** Failing tests:
  - `share dialog toggles comments` (asserts the PUT body `{on:false}`);
  - `comment button disabled when comments off`;
  - `sidebar has no composer when comments off`;
  - `appByline names editor and owner`;
  - `Get in Clax shows only for apps`;
  - e2e `comments switch refuses a guest comment`.
- [ ] **Step 2:** FAIL → implement → PASS (`just test-affected`, `just web-test`, `just web-e2e`). Raise a bundle budget by measured cost only if needed.
- [ ] **Step 3:** Commit.

---

### Task 7: Dart interfaces: remote client, linked records, sync state, `linked` trust

Spec §5.1, §5.4, §5.8. Risk: high (trust). **Commit interfaces first.**

**Files:**
- Create: `apps/clax_app/lib/remote/remote_api.dart`, `lib/remote/models.dart`, `lib/remote/sync_state.dart`, `lib/remote/fake_remote_api.dart`
- Modify: `lib/bundle/lineage_store.dart` (`LineageEntry.link: AppLink?`, JSON key `link`), `lib/bundle/trust.dart` (`TrustAnswer.linked`), `lib/bundle/updater.dart` (`_mismatch` accepts a linked entry whatever the key), `lib/bundle/installer.dart` (`open(bytes, {BundleOrigin? origin})`; install accepts `linked`)
- Test: `test/remote/sync_state_test.dart`, `test/bundle/trust_test.dart`, `test/bundle/updater_test.dart`, `test/remote/remote_api_test.dart`

**Interfaces (Produces):**
- `lib/remote/models.dart`:
  - `class Remote {String id, origin, name, status; String? personName}`
  - `class AppLink {String remoteOrigin; String? remoteId; String remoteArtifactId; String url; int baseRemoteVersion; int? takenBundleVersion; bool autoPublish; bool autoUpdate; String role /* author|recipient */}`: JSON keys snake_case.
  - `class PublishedBy {String name; String? login}`
  - `class RemoteHead {String artifactId, url, title; int version; String contentSha256; PublishedBy publishedBy, owner; bool comments; RemoteBundle? bundle}`
  - `class RemoteBundle {int version; String lineage; int bundleVersion; String contentSha256, url; PublishedBy publishedBy}`
  - `class RemoteApp {String artifactId, title, url, access, general; int version, bundleVersion}`
  - `class Sharing {…}`, mirroring `web/shell/src/hosted/api.ts` `Sharing` plus `comments`
  - `class BundleOrigin {String remoteOrigin; String remoteArtifactId; int remoteVersion; RemoteHead head}`
- `lib/remote/remote_api.dart`: `abstract interface class RemoteApi` with:
  - `Future<List<Remote>> remotes()`
  - `Future<Remote> addRemote(String url, {String kind = 'clax-app', String? machine})`
  - `Future<Remote> poll(String rid)`
  - `Future<void> removeRemote(String rid)`
  - `Future<PushResult> publish(String aid, {required int version, String? rid})`: throws `RemoteMoved(int remoteVersion)` or `UsesAssets`
  - `Future<void> putBundle(String aid, String rid, int remoteVersion, Uint8List bytes, {required String lineage, required int bundleVersion, required String contentSha256})`
  - `Future<void> link(String rid, String aid, RemoteApp app, int remoteVersion)`
  - `Future<RemoteHead> head(String rid, String raid)`
  - `Future<RemoteHead> anonymousHead(Uri artifactUrl)`
  - `Future<Uint8List> bundle(String rid, String raid, int n)`
  - `Future<Uint8List> anonymousBundle(Uri url)`
  - `Future<Map<String, Uint8List>> versionFiles(String rid, String raid, int n)`
  - `Future<List<RemoteApp>> apps(String rid)`
  - `Future<Sharing> sharing(String rid, String raid)`
  - `Future<void> share(String rid, String raid, String to, String access)`
  - `Future<void> unshare(String rid, String raid, String sid)`
  - `Future<void> setLink(String rid, String raid, String? access, {bool listed = false})`
  - `Future<void> setComments(String rid, String raid, bool on)`
  - `Future<void> unpublish(String rid, String raid)`
  - `Future<void> recordInstall(Uri artifactUrl, String installId)`

  `HttpRemoteApi` implements it over `ClaxTransport.send` for the core routes (Task 4). The `anonymous*` and `recordInstall` methods use a `dart:io` `HttpClient` capped at 32 MiB. `FakeRemoteApi` keeps in-memory remotes and throws on any unprogrammed call.
- `lib/remote/sync_state.dart`:
  - `enum SyncState { onlyHere, live, ahead, updateWaiting, keptMine, diverged }`
  - `SyncState syncState({required AppLink? link, required String liveContentSha256, required LineageEntry? entry, required RemoteHead? head, required bool modified})`. It is pure and follows the spec §5.4 table:
    - a recipient never gets `ahead`;
    - a web-made head (`head.bundle?.version != head.version`) counts as moved for authors (`diverged` when local is ahead, otherwise `updateWaiting`, so take-theirs is offered) and is invisible to recipients;
    - `updatesStopped` → `keptMine`.
- `trust.dart`: `decide(..., {BundleOrigin? origin})`. `linked` is answered before every other row when the origin's `remoteArtifactId` matches an entry's `link`, and then maps to update, current or older by `bundleVersion` against `entry.version`.

- [ ] **Step 1:** Commit the interfaces (abstract class, models, enum, signatures; implementations throw `UnimplementedError`) as `Add the app's remote client and sync-state interfaces`.
- [ ] **Step 2:** Failing tests:
  - `sync_state_test.dart`: a table test of every row in §5.4, plus a property test over random sequences of `publish`, `editLocal`, `remotePublishBundle`, `remotePublishWeb`, `keepMine` and `update`. It asserts: never `ahead` for a recipient; `diverged` only when both moved; never ↓ for a head ≤ held. Prove its reach by deleting each guard.
  - `trust_test.dart`: `linked_update_from_other_key_is_update`, `linked_same_bundle_version_is_current`, `unlinked_other_key_still_foreign`.
  - `updater_test.dart`: `linked_update_with_new_key_lands`.
  - `remote_api_test.dart`: `http_remote_api_maps_moved_and_uses_assets`.
- [ ] **Step 3:** FAIL → implement → PASS (`just test-affected`). Commit.

---

### Task 8: Connect

Spec §5.2. Risk: medium.

**Files:**
- Create: `lib/ui/remote/connect_sheet.dart`
- Modify: `lib/ui/settings_screen.dart` (Sharing section: "clax.dev" row with Connect / "Connected as @login" / Disconnect, and "Publishing as" row), `lib/platform/external_links.dart` (`openAuth(Uri)`: an in-app browser session where the platform offers one (iOS `ASWebAuthenticationSession` and Android Custom Tab through the existing `openExternal` channel with a `mode: "auth"` argument), else the browser), `lib/app.dart` (wire `RemoteApi`)
- Test: `test/ui/connect_sheet_test.dart`, `test/ui/settings_sharing_test.dart`

**Interfaces:**
- **Consumes:** `RemoteApi.addRemote/poll/removeRemote`.
- **Produces:** `Future<Remote?> showConnectSheet(BuildContext, RemoteApi, {String url = 'https://clax.dev', required Clock clock})`. It polls every `interval` on the clock, and on 10-minute expiry shows "That code expired. Try again.". The copy is:
  - "clax.dev is open in your browser. Sign in there, then check that it shows this code:";
  - "This device will be listed as Clax on <device>. You can disconnect it from either side at any time.";
  - "Open clax.dev again";
  - "Cancel".

- [ ] **Step 1:** Failing widget tests:
  - `connect_shows_code_and_connects_on_poll`;
  - `connect_expired_offers_retry`;
  - `disconnect_removes_remote`.

  Use FakeClock and FakeRemoteApi.
- [ ] **Step 2:** FAIL → implement → PASS. Commit.

---

### Task 9: Publish, Versions marks, Overview card, Unpublish, Get (pull) and library

Spec §5.3, §5.5 (in-app), §5.13. Risk: high.

**Files:**
- Create: `lib/remote/publisher.dart`, `lib/remote/puller.dart`, `lib/ui/remote/publish_sheet.dart`, `lib/ui/remote/published_sheet.dart`, `lib/ui/remote/library_screen.dart`
- Modify: `lib/ui/launcher.dart` (`HomeFlows._menu`: "Publish to clax.dev…" above share-app; `_act`), `lib/home/home_host.dart` plus `home/home.js` (menu item id `publish-remote`; "Get my apps from clax.dev" on the empty home and in ⋮ beside Import app…), `lib/ui/app_inspector.dart` (`_Overview`: an "On clax.dev" card with pill, link, Share…, New versions (Ask me | Automatically), Unpublish…), `lib/ui/versions_tab.dart` (`VersionsTab.marks: Map<int, VersionMark>`, `enum VersionMark {here, public, wasPublic}`, labels "here", "☁ public", "☁ was public"; Publish on a row), `lib/ui/bundle_sheets.dart` (`InstallSheet` provenance "From @owner on clax.dev", fingerprint under Details), `lib/app.dart`
- Test: `test/remote/publisher_test.dart`, `test/remote/puller_test.dart`, `test/ui/publish_sheet_test.dart`, `test/ui/library_screen_test.dart`, `test/ui/versions_tab_test.dart`

**Interfaces:**
- **Consumes:** Task 7, `Exporter.export`, `Installer.open/install`, `LineageStore.update`.
- **Produces:**
  - `class Publisher { Future<PublishOutcome> publish(String aid, {required int version, bool includeData = false}) }`. Steps:
    1. export the bundle (`ExportMode.own`, lineage per spec §5.8, bundle version continuing from `exportVersion`);
    2. `RemoteApi.publish`;
    3. `putBundle`, retried with backoff on the injected clock until it lands, and resumed on next launch from a pending record in `LineageEntry.link`;
    4. update `AppLink.baseRemoteVersion`.

    `sealed PublishOutcome = Published(url, remoteVersion) | Moved(remoteVersion) | Refused(message)`.
  - `class Puller { Future<PullOutcome> pull(Uri artifactUrl); Future<PullOutcome> pullFromLibrary(String rid, RemoteApp app) }`. It fetches the head, then the bundle; checks that `Bundle.manifest.contentSha256 == head.bundle.contentSha256`; calls `Installer.open(bytes, origin:)` and then install or update; links (`RemoteApi.link` when signed in, else a URL-only `AppLink`); and `recordInstall` with a random per-install ID kept in `AppSettings.installId`.
  - Publish sheet copy:
    - "Publish <title>"
    - "to clax.dev as @login"
    - "Who can open it" / "Only you, until you share it" / "🔒 Restricted"
    - "Include my data"
    - "Publish new versions" (Ask me | Automatically)
    - "Comments" / "Off: nobody can comment, whatever their access"
    - "Goes up: v<N> · <files> files · <size>"
    - "Stays here: your data, earlier versions, threads, API key"
    - "Publish"
  - Published sheet copy: "<title> is on clax.dev", "v<N> · 🔒 only you so far", the address, "Share…", "Open on clax.dev".
  - `UsesAssets` copy: "This app uses files Clax can’t share yet."

- [ ] **Step 1:** Failing tests:
  - `publish_exports_pushes_uploads_and_links`;
  - `publish_bundle_upload_resumes_after_restart`;
  - `publish_moved_returns_moved`;
  - `pull_refuses_bundle_whose_hash_differs_from_head` (Review Focus 3);
  - `pull_installs_links_and_records_install`;
  - `library_lists_and_pulls_selected`;
  - `versions_tab_marks_public`;
  - `menu_has_publish_above_share`.
- [ ] **Step 2:** FAIL → implement → PASS. Commit.

---

### Task 10: Linked-app checker and the later-version and update offers

Spec §5.3 (Rules), §5.7. Risk: medium.

**Files:**
- Create: `lib/remote/checker.dart`
- Modify: `lib/app.dart` (`didChangeAppLifecycleState.resumed` → `checker.onForeground()`; on opening a linked app → `checker.onOpen(aid)`), `lib/ui/app_screen.dart` (pill state `ClaxPillState.ahead` "↑ v<N> here · v<M> public"; toast "Claude made v<N>: <note>" with action "Publish"; Update offer), `lib/ui/working_chip.dart` (new pill state), `lib/ui/bundle_sheets.dart` (`UpdateSheet` names the publisher: "v<N> by @editor · <title> by @owner"; switch "Update <title> automatically" / "Asks first if it needs a new permission"), `lib/remote/publisher.dart` (auto-publish when `AppLink.autoPublish` after a change lands)
- Test: `test/remote/checker_test.dart`, `test/ui/app_screen_remote_test.dart`

**Interfaces:**
- **Produces:**
  - `class LinkedChecker { LinkedChecker({required RemoteApi api, required LineageStore store, required Clock clock}); ValueListenable<Map<String, LinkedStatus>> get status; Future<void> onForeground(); Future<void> onOpen(String aid); Future<void> checkNow(String aid); }`
  - `class LinkedStatus {SyncState state; RemoteHead? head; int? localVersion}`
  - At most one fetch per remote per 15 minutes on foreground (Global Constraints); `onOpen` and `checkNow` bypass the limit for that app.
  - `autoUpdate` runs `Updater.update` itself unless `addedCapabilities` is non-empty.
- **Background refresh:** WorkManager and BGAppRefresh are out of scope for this plan. Record a qualifier `concern` on `checker.dart` citing spec §5.7.

- [ ] **Step 1:** Failing tests:
  - `checker_rate_limits_per_remote_on_injected_clock`;
  - `checker_open_bypasses_limit`;
  - `checker_offline_keeps_state_and_never_regresses` (Review Focus 5);
  - `auto_update_stops_for_new_capability`;
  - `ahead_pill_and_publish_toast_after_change`.
- [ ] **Step 2:** FAIL → implement → PASS. Commit.

---

### Task 11: Native Share sheet

Spec §5.6. Risk: medium.

**Files:**
- Create: `lib/ui/remote/share_sheet.dart`
- Modify: `lib/ui/launcher.dart` (menu "Share…" for linked apps), `lib/ui/app_inspector.dart` (Share… opens it)
- Test: `test/ui/share_sheet_test.dart`

**Interfaces:**
- **Consumes:** `RemoteApi.sharing/share/unshare/setLink/setComments`.
- **Produces:** `Future<void> showShareSheet(BuildContext, RemoteApi, {required String rid, required String raid, required String title})`. The sheet has:
  - an add field ("@github-login or a name") with Can view, Can comment or Can edit (default Can view);
  - People with access;
  - General access (Restricted, Anyone with the link (view or comment), Public when `public_gallery`);
  - the Comments switch above the roles;
  - Copy link and the platform share (share_plus), enabled once anyone but the owner can open it;
  - Done.

  Labels match `web/shell/src/ui/share-model.ts` exactly.

- [ ] **Step 1:** Failing tests:
  - `share_sheet_adds_person_and_changes_access`;
  - `comments_switch_calls_setComments`;
  - `copy_link_disabled_while_restricted_alone`.
- [ ] **Step 2:** FAIL → implement → PASS. Commit.

---

### Task 12: Deep links: Get in Clax

Spec §5.5 (links). Risk: medium (platform).

**Files:**
- Modify: `android/app/src/main/AndroidManifest.xml` (on `BundleReceiverActivity`: a `VIEW` intent filter for `https://clax.dev/a/*/get` with `autoVerify="true"`, plus scheme `clax` host-less), `android/app/src/main/kotlin/dev/empathic/clax_app/BundleIntake.kt` (send `{url}` events on the bundles channel), `ios/Runner/Info.plist` (`CFBundleURLTypes` scheme `clax`), `ios/Runner/Runner.entitlements` (create: `com.apple.developer.associated-domains` = `applinks:clax.dev`; wire in the Xcode project), `ios/Runner/BundleIntake.swift` (forward `clax:` URLs and universal-link user activities as `{url}`), `macos/Runner/Info.plist` (`CFBundleURLTypes` `clax`), `macos/Runner/AppDelegate.swift` (`application(_:open:)` URLs → `{url}`), `lib/platform/bundle_intake.dart` (`opened` events may carry `url`; `BundleIntakeEvent.url(Uri)`), `lib/app.dart` (`_startBundles`: a url event → `Puller.pull`)
- Test: `test/platform/bundle_intake_test.dart`, `test/ui/bundle_entry_points_test.dart`

**Interfaces:**
- **Accepted URLs:** `clax:get?url=<https artifact URL>` and `https://<host>/a/<id>/get`. Anything else is refused with "That link isn’t a Clax app.".

- [ ] **Step 1:** Failing tests:
  - `intake_parses_clax_get_and_https_get`;
  - `intake_refuses_other_urls`;
  - `url_event_pulls`.
- [ ] **Step 2:** FAIL → implement → PASS. Run `just test-affected`, plus the app's Bazel build targets for Android and macOS (`bazel build //apps/clax_app:...` as `docs/bazel.md` "The Clax app" lists). Commit.
- **Not verifiable here:** universal and app links need clax.dev's well-known files deployed and a signed build. Record a qualifier `concern` tagged `status:needs-decision` for the owner, covering the Apple team ID, the Android signing fingerprint and the redeploy.

---

### Task 13: Comments come home

Spec §5.11. Risk: medium.

**Files:**
- Modify: `lib/clax/models.dart` (`ThreadComment.remoteOrigin: String?`, parsed from the field the core serves for mirrored comments; add it to the core's thread JSON in `crates/clax-server/src/routes/threads.rs` if it is not served), `lib/ui/thread_detail.dart` (source tag "clax.dev" beside the author), `lib/ui/threads_panel.dart` (tag in the row meta), `lib/ui/app_screen.dart` (no composer when the linked head has `comments == false`), `lib/agent/worker.dart` (a reply on a remote thread whose fix is a local unpublished version is held; `Publish v<N> and reply` publishes through `Publisher` with `addresses`, then posts)
- Test: `test/ui/thread_detail_remote_test.dart`, `test/agent/publish_and_reply_test.dart`, plus a Rust test in `crates/clax-server/tests/api_linked.rs` if the field is added

**Interfaces:**
- **Produces:** `Future<void> publishAndReply(String aid, String threadId, String text, {required int version})` on `Publisher`.

- [ ] **Step 1:** Failing tests:
  - `remote_comment_shows_source_tag`;
  - `composer_hidden_when_remote_comments_off` (Review Focus 4, app side);
  - `publish_and_reply_publishes_then_replies`;
  - `reply_without_new_version_posts_at_once`.
- [ ] **Step 2:** FAIL → implement → PASS. Commit.

---

### Task 14: Modified copies, merge, divergence, forks and badges

Spec §5.4, §5.7 (modified copy, failure), §5.10, §5.12. Risk: high.

**Files:**
- Create: `lib/remote/merge.dart`, `lib/ui/remote/diverged_sheet.dart`
- Modify: `lib/agent/loop.dart` (`Future<MergeResult> merge(String aid, {required Map<String, Uint8List> mine, required Map<String, Uint8List> theirs, required Map<String, Uint8List> base, String? session, Future<void>? cancelled})`: runs the change tools on a draft seeded with `mine`, with a prompt from `lib/agent/prompt.dart` `mergePrompt(...)` that keeps the person's changes and takes theirs; then the draft is inspected and tested like an update), `lib/ui/bundle_sheets.dart` (`UpdateChoice.bringIntoMine` "Bring v<N> into mine", shown only with a key; result offer "Your version with v<N>" with Publish to this app / Discard), `lib/ui/bundle_sheets.dart` `updateCheckFailed` (adds "Tell @owner" when comments are on and the role allows; the composer is prefilled with the reason and version and shown before sending), `lib/remote/publisher.dart` (`publishAsNewApp(aid)`: new lineage, `forked_from`, relink; `publishFork(aid, {name})`), `lib/home/home_host.dart` (`HomeApp.sync: String?`) plus `home/home.js` `TileBadge` plus `lib/ui/launcher.dart` `_AppTile` (badges ☁ live, ↑ ahead, ↓ update in the warm colour, ⑂ diverged; none for only-here and kept-mine), `lib/ui/app_inspector.dart` (Kept mine shown on Overview with "Check for updates")
- Test: `test/remote/merge_test.dart`, `test/ui/diverged_sheet_test.dart`, `home_unit/badge.test.js`, `test/ui/bundle_sheets_test.dart`

**Interfaces:**
- **Consumes:** `RemoteApi.versionFiles` (take theirs from a web head), `Updater.update`, `Publisher`.
- **Produces:**
  - `sealed MergeResult = Merged(draftId) | MergeFailed(reason)`
  - `Future<DivergeOutcome> resolveDiverged(String aid, DivergeChoice choice)` with `enum DivergeChoice {combine, takeTheirs, newApp}`
  - Diverged sheet copy:
    - "clax.dev moved on"
    - "<who> published v<N> (“<note>”) after this device’s v<M>."
    - "Ask Claude to combine them"
    - "Take the public v<N> (drop mine)"
    - "Publish mine as a new app"

- [ ] **Step 1:** Failing tests:
  - `merge_runs_on_draft_and_checks` (fake agent);
  - `bring_into_mine_hidden_without_key`;
  - `take_theirs_from_web_head_reads_files_and_publishes_locally`;
  - `combine_publishes_head_plus_one`;
  - `new_app_relinks_with_forked_from`;
  - `tell_owner_prefills_and_needs_comments_on`;
  - `tile_badges_per_state` (JS and Dart).
- [ ] **Step 2:** FAIL → implement → PASS. Commit.

---

### Task 15: End to end: publish, pull, republish, update

Spec §8. Risk: medium.

**Files:**
- Create: `crates/clax-server/tests/app_push_pull_e2e.rs`, `apps/clax_app/test/remote/real_core_push_pull_test.dart` (uses `test/support/real_core.dart` against a hosted `TestServer` started by the test's Bazel `data` dependency, or skipped with a recorded concern if the harness cannot start one)
- Test as listed.

- [ ] **Step 1:** Rust: two local homes and one hosted.
  1. A publishes in app mode and uploads a bundle.
  2. B links and fetches the bundle.
  3. A publishes again.
  4. B's head shows the new bundle.
  5. An editor C publishes on the web.
  6. A's next app push is `remote_moved`.
- [ ] **Step 2:** Dart against the real core: `Publisher.publish` then `Puller.pull` into a second home's `Installer`, then republish, then `LinkedChecker.checkNow` reports `updateWaiting`.
- [ ] **Step 3:** PASS → commit.

---

### Task 16: Documentation

Spec §6. Risk: low.

**Files:** `docs/contract.md` ("Sharing apps": linked apps, the `linked` trust answer, the two counters, bundle and head routes, Tell @owner, the end of "the core adds no route for it"; "Publishing to a public Clax": app mode, device kind, sharing for person-device tokens, the comments switch, per-version publisher, installs; Tools table unchanged), `docs/usage.md` (Clax app publish/share/get; clax.dev Get in Clax; `[hosted] app_links` and `stores` config), and the spec's corrections (§5.9 storage as a table plus content store, §5.6 auth path, migration 48).

- [ ] **Step 1:** Write. Run `just test-affected`: `scripts/test-plugins.sh` checks the contract's tool lists, which are unchanged. Commit.

---

## After the tasks

1. Four specialist reviews at once (Rust core/store and hosted; Dart app; web; native/build), then one cross-cutting pass. Findings go into qualifier; fix real defects in one round.
2. Run `just check` once. A lane that fails in code the branch did not touch is rerun alone, and both results are reported.
3. Run `qualifier diff main`.
4. The branch stays unsigned and unpushed for the owner.
