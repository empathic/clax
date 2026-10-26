# Quickstart Onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Clax features the quickstart landing page needs (spec items B to G) on the `quickstart` branch, one commit per task.

**Architecture:** Each item is a vertical slice through the daemon (Rust: `clax-store`, `clax-server`, `clax-core`), the web shell and bridge (TypeScript/Svelte under `web/`), the contract (`web/contract/clax-extensions.d.ts`, `docs/contract.md`, the four `clax` skills) and the quickstart page (`web/quickstart/`). B and C are planned to the step; D to G are scoped here and planned to the step when reached, after the owner answers the spec's open questions.

**Tech Stack:** Rust (axum, rusqlite), Svelte 5 + TypeScript (vitest, Playwright), Bazel.

**Spec:** `docs/superpowers/specs/2026-10-26-quickstart-onboarding-design.md`

## Global Constraints

- Build and test only through Bazel recipes: `just test-affected` while working, `just check` before merge. Never `cargo build|test` or `pnpm install` in the worktree (AGENTS.md).
- No `unsafe`; no hard links; tests use injected clocks, no sleeps or wall-clock budgets.
- Migrations: B1 takes 55, C1 56, D 57 (the next free on main); renumber if main moves (AGENTS.md "Homes, daemons and git").
- Doc comments and commit messages state the contract or change; write "ID" in prose.
- A capability added to the contract appears word-for-word in `docs/contract.md` "## Runtime capabilities" and all four `plugins/*/skills/clax/SKILL.md` (checked by `scripts/test-plugins.sh`), and `scripts/sync-app-prompt.sh` re-run.
- Before editing a file, `qualifier threads <path>`; record risks and deferred work as qualifier records, not TODOs.
- After each task: append a line to the matching idea's `log` in the quickstart artifact's db (`extensions/<id>`), with `work.commit`.

## Review Focus

- A `return_to` that is not loopback (`https://evil.example/`, `http://localhost.evil.example/`, `http://user@localhost/`, `javascript:`): refused with 400 `invalid_args`, never stored.
- A public Clax older than B, which rejects `return_to` (`deny_unknown_fields` → 400 `invalid_json`): local sign-in still works, falling back to a new tab.
- A publish whose `clax.json` is malformed JSON, not an object, or carries an unknown top-level key: 400 `invalid_manifest`, nothing published.
- Restoring an older version: the page gets that version's declaration, not the current one.
- Returning from sign-in with `publish=1` but no pending remote (expired, or signed in from another tab meanwhile): the dialog opens in its normal state, no error.

---

### Task B1: Hosted device sign-in accepts a loopback `return_to`

**Files:**
- Modify: `crates/clax-store/src/migrations.rs` (migration 55: `ALTER TABLE device_codes ADD COLUMN return_to TEXT;`, its `Mark`, `RETURN_TO_MIGRATION` const, sequence tests)
- Modify: `crates/clax-store/src/device.rs` (`start_device_kind` takes `return_to: Option<&str>`; `DeviceRequest.return_to: Option<String>`)
- Create: `crates/clax-http/src/loopback.rs` (`pub fn loopback_return(url: &str) -> Option<String>`)
- Modify: `crates/clax-server/src/hosted.rs:1207-1263` (`DeviceCodeBody.return_to`, validation, pass through)
- Test: `crates/clax-http/src/loopback.rs` (unit), `crates/clax-server/tests/api_hosted.rs` (device section, after :5181)

**Interfaces:**
- Produces: `clax_http::loopback::loopback_return(url: &str) -> Option<String>`: the URL normalised when its scheme is `http`, host `localhost` | `127.0.0.1` | `[::1]`, it has no userinfo or fragment, and it is at most 2048 bytes; else `None`.
- Produces: `GET /api/hosted/device/{code}` answers `return_to: string | null`.

- [ ] **Step 1: Write the failing unit test** `loopback_return_accepts_only_this_machine`: accepts `http://localhost:7480/a/x?publish=1`, `http://127.0.0.1:7481/`, `http://[::1]:7480/a/x`; refuses `https://localhost/`, `http://localhost.evil.example/`, `http://user@localhost/`, `http://localhost/#f`, `javascript:alert(1)`, a 2049-byte URL.
- [ ] **Step 2: Run** `bazel test //crates/clax-http:unit --test_filter=loopback` and see it fail (no module).
- [ ] **Step 3: Implement `loopback_return`** with the `url` crate already in `clax-http`'s deps.
- [ ] **Step 4: Write the failing integration test** `a_device_sign_in_carries_a_loopback_return_to_and_refuses_any_other`: POST `/api/hosted/device/code {client_name, return_to: "http://localhost:7480/a/x?publish=1"}` → 200; the member's `GET /api/hosted/device/<user code>` has that `return_to`; with `return_to: "https://evil.example/"` → 400 `invalid_args`.
- [ ] **Step 5: Add migration 55 and thread `return_to` through `start_device_kind` → INSERT → `peek_device`'s `DeviceRequest`.**
- [ ] **Step 6: Run** `just test-affected` and see the device tests and migration sequence tests pass.
- [ ] **Step 7: Commit** `Let a device sign-in name a loopback page to return to once decided`.

### Task B2: The approval page returns the person to their Clax

**Files:**
- Modify: `web/shell/src/hosted/api.ts:76-80` (`DeviceRequest.return_to: string | null`)
- Modify: `web/shell/src/hosted/SignIn.svelte:77-84,176-197` (after approve or deny, `location.assign(withParam(return_to, "clax_signin", "approved" | "denied"))`; the waiting state says "Approve, then go back to <host:port>")
- Test: `web/shell/src/hosted/SignIn.test.ts`

**Interfaces:**
- Consumes: B1's `return_to` in the peek answer.

- [ ] **Step 1: Write failing tests** `approving_returns_to_the_loopback_page` (peek answers `return_to: "http://localhost:7480/a/x?publish=1"`; after Approve, `location.assign` was called with `http://localhost:7480/a/x?publish=1&clax_signin=approved`) and `denying_returns_too` (`…&clax_signin=denied`); with `return_to: null` the page keeps today's "You can close this tab".
- [ ] **Step 2: Run** `bazel test //web:unit` and see them fail.
- [ ] **Step 3: Implement** in `SignIn.svelte`, re-checking the URL client-side with the same loopback rule (`web/shell/src/hosted/next.ts` gains `loopbackReturn(url): string | null`).
- [ ] **Step 4: Run** `bazel test //web:unit //web:typecheck //web:lint` and see them pass.
- [ ] **Step 5: Commit** `Send the person back to their Clax after they approve or deny its sign-in`.

### Task B3: Local sign-in passes `return_to`, and Publish uses the same tab

**Files:**
- Modify: `crates/clax-server/src/routes/remotes.rs:103-120,130-205` (`AddBody.return_to: Option<String>`, a path checked with `safe_next`, made absolute against the request's loopback shell origin)
- Modify: `crates/clax-server/src/public.rs:743-800` (`start_sign_in(origin, name, kind, return_to: Option<&str>)`; on 400 `invalid_json` naming `return_to`, retry once without it)
- Modify: `web/shell/src/ui/publish-api.ts` (`startSignIn(url, returnTo?)`)
- Modify: `web/shell/src/ui/Publish.svelte` (same-tab: `nav.assign(verification_url)` with `returnTo = here + "?publish=1"`; on load, `publish=1&clax_signin=approved` → poll once, `refresh()`, open the dialog; `denied` → open it with the declined message; drop both parameters with `setUrl`)
- Modify: `crates/clax-testkit/src/remotes.rs` (`connect_with` accepts an optional `return_to`)
- Test: `crates/clax-server/tests/api_remotes.rs`, `crates/clax-server/tests/api_remotes_app.rs` (old remote), `web/shell/src/publish.test.ts`
- Docs: `docs/contract.md` "Signing in, by device code", `docs/usage.md` Publishing paragraph

**Interfaces:**
- Consumes: B1's hosted `return_to`.
- Produces: `POST /api/remotes {url, return_to?}`; `return_to` a path (`/a/<id>?publish=1`), 400 `invalid_args` otherwise.

- [ ] **Step 1: Write failing Rust tests** `a_sign_in_names_the_page_to_return_to` (the hosted peek shows `http://localhost:<port>/a/x?publish=1`) and `a_public_clax_without_return_to_still_signs_in` (`OldPublic` refuses the field; sign-in completes).
- [ ] **Step 2: Write failing shell tests** `publish_signs_in_in_the_same_tab` (`nav.assign` called with the verification URL; `startSignIn` got `returnTo` ending `?publish=1`) and `returning_approved_reopens_the_publish_dialog` (load with `?publish=1&clax_signin=approved`: one poll, dialog shows "Publish “Plan” on clax.dev, as Ada", address no longer has the parameters).
- [ ] **Step 3: Run** `just test-affected`; see them fail.
- [ ] **Step 4: Implement** the route, `start_sign_in` with fallback, and the shell.
- [ ] **Step 5: Run** `just test-affected`; see them pass. Update both docs.
- [ ] **Step 6: Commit** `Sign in from Publish in the same tab and come back to the page`.

### Task C1: Declarations kept per version

**Files:**
- Modify: `crates/clax-store/src/migrations.rs` (migration 56: `ALTER TABLE versions ADD COLUMN capabilities_json TEXT;` backfilled for each artifact's current version from `artifacts.capabilities_json`)
- Modify: `crates/clax-store/src/artifacts.rs:1228-1293,1657,1951-1969,1491` (write the declaration on each version; restore takes the restored version's, falling back to the current one when NULL)
- Modify: `crates/clax-store/src/forks.rs:209-218` (version 1 of a fork gets the source version's declaration)
- Test: `crates/clax-store/src/artifacts.rs` tests, `crates/clax-server/tests/api_publish.rs` (or the file holding restore tests; find with `grep -l restore crates/clax-server/tests`)

**Interfaces:**
- Produces: `Store::version_capabilities(aid: &str, n: u32) -> Result<serde_json::Value>`.

- [ ] **Step 1: Write failing tests** `each_version_keeps_its_declaration` (publish v1 `{db:{}}`, v2 `{db:{}, net:{hosts:["a.example"]}}`; `version_capabilities(1)` is v1's) and `restore_takes_the_restored_versions_declaration`.
- [ ] **Step 2: Run** `just test-affected`; see them fail.
- [ ] **Step 3: Implement** migration 56 and the writes.
- [ ] **Step 4: Run** `just test-affected`; see them pass.
- [ ] **Step 5: Commit** `Keep each version's capability declaration, and restore it with the version`.

### Task C2: `clax.json`, the manifest

**Files:**
- Create: `crates/clax-core/src/manifest.rs` (`pub const MANIFEST_PATH: &str = "clax.json"`; `pub fn parse(bytes: &[u8]) -> Result<Value>` returning the `capabilities` object; `pub fn render(caps: &Value) -> Vec<u8>`, two-space JSON with a trailing newline)
- Modify: `crates/clax-core/src/publish.rs:168-188,641-746` (`check_declared` reads `clax.json` when the publish carries it; a differing `capabilities` argument is 400 `manifest_mismatch`; a `capabilities` argument without `clax.json` adds the rendered file to the version)
- Modify: `docs/contract.md` (publish arguments, "Runtime capabilities": declare in `clax.json`), the four skills' identical section, `scripts/sync-app-prompt.sh` run
- Test: `crates/clax-core/src/manifest.rs` (unit), `crates/clax-server/tests/api_publish.rs`, `crates/clax-mcp/tests/` publish test

**Interfaces:**
- Consumes: C1's per-version storage.
- Produces: error codes `invalid_manifest`, `manifest_mismatch`.

- [ ] **Step 1: Write failing unit tests** `a_manifest_is_an_object_with_only_capabilities` (accepts `{"capabilities":{}}`; refuses `[]`, `{"capabilities":[]}`, `{"capabilities":{},"x":1}`, invalid UTF-8, invalid JSON, each `invalid_manifest`) and `render_round_trips`.
- [ ] **Step 2: Write failing integration tests** `a_publish_with_clax_json_declares_from_it`, `a_capabilities_argument_that_differs_from_clax_json_is_refused`, `a_capabilities_argument_alone_writes_clax_json` (version files list `clax.json`, its bytes are `render(caps)`), `clax_json_carries_forward_when_a_publish_names_neither`.
- [ ] **Step 3: Run** `just test-affected`; see them fail.
- [ ] **Step 4: Implement** `manifest.rs` and the publish changes.
- [ ] **Step 5: Run** `just test-affected` (includes `scripts/test-plugins.sh` for the skills); see them pass.
- [ ] **Step 6: Commit** `Declare an artifact's capabilities in clax.json, versioned with its files`.

### Task D (scoped; detail after C): one permissions system

Spec §7. Migration 57 `capability_approvals`; port `addedCapabilities` (`apps/clax_app/lib/bundle/updater.dart:41`) to Rust as `clax_core::capabilities::added(live, next)`; the shell serves the last approved declaration until the owner approves (`web/shell/src/caps/availability.ts`, `view/artifact-controller.ts:718`); an approval bar; a Permissions panel in `MoreMenu.svelte` over `net-grants`, `sample-grant`, `db-grants` and the new approvals. Every artifact's first declaration asks (Q4 decided).

### Task E (scoped; detail after D): the `account` capability

Spec §5. Bridge `web/bridge/src/caps/account.ts` + `capabilities.ts` `CAPABILITY_METHODS.account = ["status","onChange","signIn"]`; shell `web/shell/src/caps/account.ts` (consent through D's grants, local device sign-in reusing B3's same-tab flow with `account=1`, hosted `/auth/github?next=`), `registry.ts`, `availability.ts` (`account`, `needsAccount`); `clax-core::capabilities::validate` checks `account.origins`; contract and skills. Waits on Q1 to Q3.

### Task F (scoped): the `shell` capability

Spec §8. `ClaxExtensions.Shell` (`highlight`, `commentMode`, `can`); shell handler driving `ArtifactController.toggleComment` (:1656) with the strict gesture (`caps/gesture.ts` `frameGestureStrict`) and a coach-mark component over `TopbarIsland.svelte` controls; consent through D.

### Task G (scoped): the quickstart in every new gallery

Spec §9. `web/quickstart/` copied into `web/dist/quickstart/` by the web build (`web/BUILD.bazel` `dist_all`); a `home_flags` row and a seeding function in `clax-store`, called from the owner's first gallery listing (local) and `GitHubOutcome::New` / invite redemption (hosted); release tag and update-at-start rule, stopping at the first comment or version by anyone but Clax (Q5 decided). The page switches step 1 to `account.signIn()` and its tour to `shell.highlight`.
