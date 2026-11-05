# Quickstart D: One Permissions System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every capability that reaches past the page is in force only once the artifact's administrator approves the declaration that asks for it, per artifact and per version, and is listed and revocable in one Permissions panel (spec item D).

**Architecture:** A pure rule in `clax-core` (`capabilities::added`, ported from the Clax app, and `pending` / `in_force` over it) decides what a declaration adds over the last approved one. `clax-store` keeps one approval per artifact (migration 57) and computes the declaration in force; the daemon enforces it on the `net` and `sample` routes and stamps it on the artifact view, so the shell hands the page only what is in force. The shell shows an approval bar and a Permissions panel over one new read route; the Clax app approves what its person accepts.

**Tech Stack:** Rust (axum, rusqlite), Svelte 5 + TypeScript (vitest, Playwright), Dart (Flutter app), Bazel.

**Spec:** `docs/superpowers/specs/2026-10-26-quickstart-onboarding-design.md` (§2 D3 to D5, §6 C, §7 D; Q4 decided: every artifact's first declaration asks, the owner's and their agents' included). Builds on C1 (`versions.capabilities_json`, `Store::version_capabilities`) and C2 (`clax.json`, `clax_core::manifest`).

## Decisions

1. **What reaches past the page (gated):** `net`, `sample`, `account`, `shell`, `notify`, `share`, `file`, `camera`, `location`; not `db`, `comments`, `user`, `room`, `artifact`/`self`, `assets`, `downloads`, `home`. Rationale: the gated ones touch the network, the owner's money, sign-in, Clax's own UI or the device; the rest stay inside the page and its own data, or already have a per-viewer consent (`comments`, `user`) or an app-side rule (`home` is served only to the app's home app).
2. **The rule:** `clax_core::capabilities::added(live, next)` is a faithful port of the app's `addedCapabilities` (new top-level names, new `net` hosts); `pending(approved, next)` keeps the gated part of it. Rationale: one rule in two languages, tested on the same cases; the gate is a filter on it, not a second diff.
3. **First declarations:** with no approval, the approved declaration is `{}`, so any gated name in a first declaration is pending, whoever published it (Q4). A declaration with nothing gated needs nothing approved. Rationale: Q4 without a bar on every `db` page.
4. **Existing artifacts:** migration 57 records an approval of each live artifact's current declaration (`by = "migration"`). Rationale: those declarations are in force today; without the backfill every installed app loses `net` and `sample` on upgrade.
5. **Storage:** `capability_approvals (artifact_id PRIMARY KEY, version_n, capabilities_json, approved_at, by)`, one row per artifact holding the last approval (history is in the audit journal); `by` is `"owner"`, `"person:<ID>"` or `"migration"`. Rows go with the artifact (soft-delete trigger, foreign key), as `net_grants` do.
6. **What the page gets until approval:** the declaration *in force*: the current declaration with each pending gated name removed and `net.hosts` narrowed to approved hosts (`net` dropped if none remain). On every gated name it equals the last approved declaration; ungated additions (a new `db` collection) work at once. A draft is held to its live app's approval, as its `net` grants already are. Rationale: the spec's "serves the last approved declaration" without breaking ungated features of the new version.
7. **Where it is computed and enforced:** the daemon. `Store::capabilities_in_force` feeds `net_access` (fetch and grant), the `sample` routes, the artifact view (`capabilities_in_force`) and the site's visitor view; the shell passes `capabilities_in_force` as `declared` to `CapabilityHost` and treats a missing field as `{}`. Rationale: one source of truth; a page that bypasses the shell gets nothing more.
8. **Routes:** `GET /api/artifacts/<id>/permissions` (the panel's one read), `PUT /api/artifacts/<id>/capability-approval {version}` (approve, 409 `stale_version` unless `version` is current), `DELETE /api/artifacts/<id>/capability-approval/<name>` (withdraw one gated name). All refuse agents (`DeniedToGrants` and `refuse_agents`), page-marked requests (`x-clax-via: page`, 403 `not_for_pages`) and callers who do not administer the artifact (`clax_http::agent::admin_only`). A `permissions` event tells open views. Rationale: approving a version, not "whatever is current", keeps a racing publish from being approved unseen.
9. **Approval bar copy:** `v4 asks for: the internet (api.example.com), Claude` and a **Review** button; Review opens a dialog titled `Let “<title>” use these?` with one line per item, **Approve** and **Not now** (hides the bar for this view). Labels: net "the internet (<hosts>)", sample "Claude", account "sign-in", shell "Clax's controls", notify "notifications", share "sharing", file "files", camera "the camera", location "your location". Shown only to callers who may approve (`may_approve`).
10. **Existing consents:** kept as the per-use layer, with Approve folding in the first ask: approving grants the pending `net` hosts (`grant_net`) and, when `sample` is pending, the sample grant, in the same transaction. The runtime prompts in `caps/net.ts` and `caps/sample.ts` stay, and ask again after a revoke. `db.read` keeps its per-source cross-read consent; `comments` keeps its per-viewer browser consent. Rationale: one ask per new capability, and the spec's "revoke deletes the grant; the page asks again on its next use".
11. **Permissions panel:** a **Permissions…** item in the more menu (`may_approve` only) opens a panel with rows: waiting for approval (Review); the internet (hosts, granted or "asks on first use"; Revoke → `DELETE net-grants`); Claude (Revoke → `DELETE sample-grant`); other pages' data (one row per source; Revoke → `DELETE db-grants/<source>`); sign-in, Clax's controls and each device capability ("approved for v<n>"; Revoke → `DELETE capability-approval/<name>`); comments by the page (this browser's consent; Revoke clears it). Rationale: mirrors `app_inspector_permissions.dart` row for row, which keeps its own UI.
12. **Hosted daemons:** the artifact's maker approves (`administers`: the member for their artifact, the hosted owner for theirs); members it is shared with and world visitors get the declaration in force. A page pushed from a local Clax arrives unapproved and its maker approves it on clax.dev. Rationale: approval belongs to who answers for the page there; carrying a local approval across a push is a trust question for the owner (Questions).
13. **The Clax app:** after its person accepts an install or an update (the sheets that list `addedCapabilities`), the app calls the approve route for the version it published; automatic updates approve nothing. Rationale: one consent in the app, not a second bar in its web view.
14. **Agents:** a publish answer carries `pending_capabilities`, and the MCP `publish` result says what waits for the owner, so an agent tells the person instead of debugging a `null`.
15. **E2E proof:** `web/e2e/permissions.spec.ts` — a page declaring `net` and `db` sees `use("net") === null` and `use("db")` resolved, and `permissions.state("net") === "unavailable"`; the owner approves from the bar, the frame reloads, `use("net")` resolves and a fetch goes to the daemon with no prompt; a v2 adding a host shows the bar again while v1's host keeps working; revoking the internet in the panel makes the next fetch ask.

## Global Constraints

- Build and test only through Bazel recipes: `just test-affected` while working, `just check` before merge. Never `cargo build|test` or `pnpm install` in the worktree (AGENTS.md).
- Agents commit with `git -c commit.gpgsign=false commit`, no attribution lines.
- No `unsafe`; no hard links; tests use injected clocks, no sleeps or wall-clock budgets.
- Migration 57 is `CAPABILITY_APPROVALS_MIGRATION = VERSION_CAPABILITIES_MIGRATION + 1`, this build's last; renumber if main moves (AGENTS.md "Homes, daemons and git"); update `crates/clax-embed/tests/old_homes.rs` as C1 did.
- Doc comments and commit messages state the contract or change; write "ID" in prose.
- Contract text added under `docs/contract.md` "## Runtime capabilities" appears word-for-word in all four `plugins/*/skills/clax/SKILL.md` (`scripts/test-plugins.sh`), and `scripts/sync-app-prompt.sh` is re-run.
- Before editing a file, `qualifier threads <path>`; record risks and deferred work as qualifier records, not TODOs.
- After each task: append a line to the `capability-manifest`/`shell-tour` idea's `log` in the quickstart artifact's db, with `work.commit`.

## Review Focus

- A publish lands while the owner reviews: `PUT capability-approval {version: 4}` after v5 → 409 `stale_version`, nothing approved, the bar names v5 (test in D3).
- A restore of an older version whose host was dropped since: the restored host is pending again, not silently back (test in D2).
- A page approving itself: a request with `x-clax-via: page`, the owner's agent (token with `X-Clax-Session`), a person's agent (`Clax-Grant`), or a hosted member who does not administer the artifact → 403, nothing approved (test in D3).
- An upgrade from a home before D: installed apps keep `net` and `sample` with no bar (migration backfill test in D2).
- A draft of an app (the Clax app's update check) declaring a new host: refused like its live app, and the approve route on a draft → 403 `draft` (test in D2 and D3).

---

### Task 1 (D1): The approval rule in `clax-core`

**Files:**
- Modify: `crates/clax-core/src/capabilities.rs` (append the rule and its tests)

**Interfaces:**
- Produces:
  - `pub const GATED: &[&str] = &["net", "sample", "account", "shell", "notify", "share", "file", "camera", "location"];`
  - `#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)] pub struct Added { pub capabilities: Vec<String>, pub hosts: Vec<String> }` with `pub fn is_empty(&self) -> bool`.
  - `pub fn added(live: &Value, next: &Value) -> Added` — `next`'s top-level names absent from `live` (in `next`'s order; `net` only when `live` has none) and `next`'s `net.hosts` absent from `live`'s.
  - `pub fn pending(approved: Option<&Value>, next: &Value) -> Added` — `added(approved.unwrap_or(&{}), next)` with `capabilities` kept to `GATED` names.
  - `pub fn in_force(approved: Option<&Value>, next: &Value) -> Value` — `next` without the pending gated names, `net.hosts` narrowed to the approved hosts, `net` dropped when none remain.

- [ ] **Step 1: Write the failing tests** in `capabilities.rs`'s `mod tests`:

```rust
#[test]
fn added_is_the_clax_apps_rule() {
    // apps/clax_app/test/bundle/updater_test.dart 'addedCapabilities lists new capabilities and hosts'
    let d = added(&json!({"db": {}, "net": {"hosts": ["a.com"]}}),
                  &json!({"db": {}, "notify": {}, "net": {"hosts": ["a.com", "b.com"]}}));
    assert_eq!(d.capabilities, vec!["notify"]);
    assert_eq!(d.hosts, vec!["b.com"]);
    let first = added(&json!({}), &json!({"net": {"hosts": ["a.com"]}}));
    assert_eq!((first.capabilities, first.hosts), (vec!["net".to_string()], vec!["a.com".to_string()]));
}

#[test]
fn pending_keeps_what_reaches_past_the_page() {
    let p = pending(None, &json!({"db": {}, "comments": {}, "user": {}, "sample": {}, "shell": {}}));
    assert_eq!(p.capabilities, vec!["sample", "shell"]);
    assert!(pending(Some(&json!({"sample": {}})), &json!({"sample": {}, "db": {}})).is_empty());
}

#[test]
fn in_force_is_the_approved_declaration_on_gated_names() {
    let approved = json!({"net": {"hosts": ["a.com"]}});
    let next = json!({"db": {}, "sample": {}, "net": {"hosts": ["a.com", "b.com"]}});
    assert_eq!(in_force(Some(&approved), &next), json!({"db": {}, "net": {"hosts": ["a.com"]}}));
    assert_eq!(in_force(None, &json!({"net": {"hosts": ["a.com"]}, "db": {}})), json!({"db": {}}));
    // A host the next version drops is not in force, whatever was approved.
    assert_eq!(in_force(Some(&json!({"net": {"hosts": ["a.com", "b.com"]}})), &json!({"net": {"hosts": ["b.com"]}})),
               json!({"net": {"hosts": ["b.com"]}}));
}
```

- [ ] **Step 2: Run** `bazel test //crates/clax-core:clax_core_lib_test --test_arg=capabilities::` — Expected: FAIL to compile (`added`, `pending`, `in_force` not found).
- [ ] **Step 3: Implement** `GATED`, `Added`, `added`, `pending`, `in_force` in `capabilities.rs`; hosts read with the same shape as the Dart `hostsOf` (non-string entries skipped), not through `net_hosts` (which errors).
- [ ] **Step 4: Run** the same command — Expected: PASS (all `capabilities::` tests).
- [ ] **Step 5: Commit**

```bash
git add crates/clax-core/src/capabilities.rs
git -c commit.gpgsign=false commit -m "Port the Clax app's added-capabilities rule, and say which declarations need approval"
```

### Task 2 (D2): Approvals in the store, and the declaration in force

**Files:**
- Modify: `crates/clax-store/src/migrations.rs` (migration 57; `CAPABILITY_APPROVALS_MIGRATION`; its `Mark::Table("capability_approvals")`; sequence tests that name the last migration)
- Create: `crates/clax-store/src/capability_approvals.rs` (module declared in `crates/clax-store/src/lib.rs`)
- Modify: `crates/clax-store/src/net_grants.rs:100-131` (`own_access_in` reads the declaration in force; `NetAccess.waiting`)
- Modify: `crates/clax-embed/tests/old_homes.rs` (last migration number)
- Test: `crates/clax-store/src/capability_approvals.rs` (unit), `crates/clax-store/src/migrations.rs` (backfill)

**Interfaces:**
- Consumes: D1's `added`, `pending`, `in_force`, `Added`; C1's `Store::version_capabilities`.
- Produces:
  - Migration 57: `CREATE TABLE capability_approvals (artifact_id TEXT PRIMARY KEY REFERENCES artifacts(id) ON DELETE CASCADE, version_n INTEGER NOT NULL, capabilities_json TEXT NOT NULL, approved_at TEXT NOT NULL, by TEXT NOT NULL);` a trigger deleting the row on the artifact's soft delete (as `net_grants`' migration 44); backfill `INSERT … SELECT id, current_version, capabilities_json, <now>, 'migration' FROM artifacts WHERE deleted_at IS NULL AND capabilities_json NOT IN ('{}', '') AND <not a hidden draft>`.
  - `pub struct Approval { pub version: u32, pub capabilities: Value, pub approved_at: String, pub by: String }` (Serialize).
  - `pub struct Pending { pub version: u32, #[serde(flatten)] pub added: Added }` (Serialize).
  - `Store::capability_approval(&self, id: &ArtifactId) -> Result<Option<Approval>>`
  - `Store::capabilities_in_force(&self, id: &ArtifactId) -> Result<Value>` (a hidden draft: its declaration against its live app's approval)
  - `Store::pending_capabilities(&self, id: &ArtifactId) -> Result<Option<Pending>>` (`None` when nothing is pending)
  - `Store::approve_capabilities(&self, audit: &AuditCtx, id: &ArtifactId, version: u32, by: &str) -> Result<Approval>` — `stale_version` (409 by its `CoreError` kind, as `if_version` conflicts are) unless `version` is current; `draft` for a hidden draft; records the current declaration, grants the pending `net` hosts and, when pending, `sample`, in one transaction, one `artifact.update` audit entry.
  - `Store::withdraw_capability(&self, audit: &AuditCtx, id: &ArtifactId, name: &str) -> Result<bool>` — removes `name` from the approved declaration (`invalid_args` for a name outside `GATED`); `false` when it was not approved.
  - `NetAccess { declared, grants, waiting: Vec<String> }` — `declared` is now the hosts in force; `waiting` the declared hosts pending approval.

- [ ] **Step 1: Write the failing unit tests** in `capability_approvals.rs`:
  - `a_first_declaration_waits_for_approval`: `artifact_with_caps(&st, json!({"db": {}, "net": {"hosts": ["a.example.com"]}}))` → `capabilities_in_force == json!({"db": {}})`, `pending_capabilities` is `Some(Pending { version: 1, added: Added { capabilities: vec!["net"], hosts: vec!["a.example.com"] } })`.
  - `approving_puts_it_in_force_and_grants_its_hosts_and_sample`: after `approve_capabilities(DAEMON, &a, 1, "owner")` with `{"net": {"hosts": ["a.example.com"]}, "sample": {}}`, `capabilities_in_force` equals the declaration, `pending_capabilities` is `None`, `net_access(&a).allows("a.example.com")`, `sample_grant(&a)` is `Some(_)`.
  - `approving_a_version_that_is_not_current_is_refused`: publish v2, `approve_capabilities(.., 1, ..)` → `Err` whose code is `stale_version`; nothing recorded.
  - `a_version_that_adds_a_host_keeps_the_approved_ones`: approve v1 (`a.example.com`), v2 adds `b.example.com` → in force `{"net": {"hosts": ["a.example.com"]}}`, `net_access(&a).waiting == ["b.example.com"]`.
  - `a_restore_of_a_dropped_host_asks_again`: v1 `a`, approve; v2 drops `a` for `b`, approve; restore v1 → pending hosts `["a.example.com"]`.
  - `withdrawing_a_name_narrows_the_approval`: approve `{"shell": {}, "sample": {}}`; `withdraw_capability(.., "shell") == true`; in force has `sample` only; `withdraw_capability(.., "db")` → `invalid_args`.
  - `a_draft_is_held_to_its_live_apps_approval`: a hidden draft of an approved app adding a host → that host not in force; `approve_capabilities` on the draft → `draft`.
  - `deleting_the_artifact_removes_its_approval`.
- [ ] **Step 2: Write the failing migration test** `migration_57_approves_every_declaration_in_force` in `migrations.rs`: a database at 56 with one artifact declaring `{"net":{"hosts":["a.example.com"]}}` and one declaring `{}`; after migrating, the first has a row `(version_n = current_version, by = 'migration')`, the second none.
- [ ] **Step 3: Run** `bazel test //crates/clax-store:clax_store_lib_test` — Expected: FAIL (no module, no migration 57).
- [ ] **Step 4: Implement** migration 57, `capability_approvals.rs` (a crate-private `in_force_in(c: &Connection, id: &ArtifactId) -> Result<Value>` that `net_grants::own_access_in` calls), and the `NetAccess.waiting` field.
- [ ] **Step 5: Run** `bazel test //crates/clax-store:clax_store_lib_test //crates/clax-embed/...` — Expected: PASS, including the existing `net_grants` tests (they publish through `artifact_with_caps`; give that helper an approval so they keep their meaning, and say so in its doc comment).
- [ ] **Step 6: Commit**

```bash
git add crates/clax-store crates/clax-embed/tests/old_homes.rs
git -c commit.gpgsign=false commit -m "Keep each artifact's approved declaration, and hold what it adds out of force until approved"
```

### Task 3 (D3): The daemon enforces and serves approvals

**Files:**
- Create: `crates/clax-server/src/routes/permissions.rs` (`permissions`, `approve`, `withdraw`)
- Modify: `crates/clax-server/src/router.rs:318-327` (three routes beside `sample-grant`, available on the Worker too)
- Modify: `crates/clax-core/src/events.rs` (`Event::Permissions { artifact_id: String }`, in `artifact_id()`), `crates/clax-server/src/routes/events.rs` and `crates/clax-realtime/src/stream.rs` (carried on the artifact's topic, as `Event::Comments` is)
- Modify: `crates/clax-server/src/routes/app_bundles.rs:422` (beside `stamp_app_fields`: `pub fn stamp_capabilities(st: &Store, a: &Artifact, may_approve: bool, view: &mut Value) -> clax_core::Result<()>`), called from `crates/clax-server/src/routes/artifacts.rs:1039` and `crates/clax-server/src/shell/boot.rs:345`
- Modify: `crates/clax-server/src/routes/net.rs:73-95` (403 `not_approved` for a host in `access.waiting`)
- Modify: `crates/clax-server/src/routes/sample.rs:101,435` (declared ⇒ in force; 403 `not_approved` when declared but pending)
- Modify: `crates/clax-server/src/routes/site_api.rs:536-546` (`visitor_artifact` serves the declaration in force as `capabilities`)
- Modify: the publish answers in `crates/clax-server/src/routes/artifacts.rs` (create and new version): top-level `pending_capabilities`
- Test: create `crates/clax-server/tests/api_permissions.rs` (and `mod api_permissions;` in `tests/integration.rs`), a `approve(&daemon, id)` helper in `crates/clax-server/tests/common/`; update `crates/clax-server/tests/api_net.rs`, `api_sample.rs`, `api_restore.rs` setups; add `approve(base, token, id)` to `web/e2e/fixtures.ts` and call it after each publish that declares a gated capability in `web/e2e/{net,sample,app-mode,contract,home-app,map}.spec.ts`; rewrite `net.spec.ts`'s and `sample.spec.ts`'s first-ask tests to revoke the grant after approving (the ask they test is now the ask after a revoke)

**Interfaces:**
- Consumes: D2's store API.
- Produces:
  - `GET /api/artifacts/<id>/permissions` → `{version, declared, in_force, approved: Approval | null, pending: Pending | null, net: {declared, waiting, grants, granted}, sample: {granted, granted_at}, db_read: [{source_id, title, granted_at}]}`.
  - `PUT /api/artifacts/<id>/capability-approval` body `{version: u32}` → `{approval: Approval, in_force}`; emits `Event::Permissions`.
  - `DELETE /api/artifacts/<id>/capability-approval/<name>` → `{withdrawn: bool, in_force}`; emits `Event::Permissions`.
  - All three: 404 unknown; 403 `not_for_agents` / `forbidden` (agents, via `DeniedToGrants` + `refuse_agents()`), 403 `not_for_pages` (`page_marker`), 403 admin-only (`administers` false); PUT 409 `stale_version`, 403 `draft`.
  - Artifact view (`GET /api/artifacts/<id>`, shell boot): `capabilities_in_force` always; `may_approve` and `pending_capabilities` (`Pending | null`) for callers who administer it, absent otherwise.
  - Publish answers: `pending_capabilities: Pending | null`.

- [ ] **Step 1: Write failing integration tests** in `api_permissions.rs`:
  - `a_new_declaration_is_not_in_force_until_its_owner_approves`: publish `{net:{hosts:["api.clax.invalid"]}, db:{}}`; `GET /api/artifacts/<id>` has `capabilities_in_force == {"db":{}}`, `pending_capabilities == {"version":1,"capabilities":["net"],"hosts":["api.clax.invalid"]}`, `may_approve == true`; `POST net:fetch` → 403 `not_approved`; `PUT capability-approval {version:1}` → 200; `capabilities_in_force` equals the declaration; `GET net-grants` has `granted: true`.
  - `the_publish_answer_says_what_waits`: the create answer's `pending_capabilities.capabilities == ["net"]`.
  - `approving_a_version_that_is_not_current_is_refused`: publish v2 then `PUT {version:1}` → 409 `stale_version`.
  - `no_agent_and_no_page_approves`: with `X-Clax-Session` → 403 `not_for_agents`; with `x-clax-via: page` → 403 `not_for_pages`; a person's agent (`Clax-Grant`, via the existing people helpers) → 403; each leaves `pending_capabilities` unchanged.
  - `withdrawing_a_name_takes_it_out_of_force`: approve `{sample:{}}` then `DELETE capability-approval/sample` → `{withdrawn:true}`; `POST sample` → 403 `not_approved`.
  - `approving_tells_open_views`: an `/api/events` subscriber sees `{"type":"permissions","artifact_id":<id>}` after the PUT.
  - `a_visitor_reads_the_declaration_in_force` (site API `artifact` for a world visitor): `capabilities` lacks the pending `net`.
- [ ] **Step 2: Write the failing hosted test** in `crates/clax-server/tests/api_hosted.rs`: `a_members_page_waits_for_its_maker`: member A publishes `{shell:{}}`; member B (shared as editor) gets no `pending_capabilities` and `PUT capability-approval` → 403; A approves → B's `capabilities_in_force` has `shell`.
- [ ] **Step 3: Run** `just test-affected` — Expected: the new tests FAIL (404 on the routes).
- [ ] **Step 4: Implement** the routes, the event, the stamping, the `net`/`sample`/site-API enforcement and the publish answers' field.
- [ ] **Step 5: Update the existing tests** listed under Files to approve after publishing (Rust `approve`, e2e `approve`); find any others with `rg -l '"(net|sample|notify|share|file|camera|location)"\s*:\s*\{' crates/*/tests web/e2e apps/clax_app/test/**/*_real_test.dart`.
- [ ] **Step 6: Run** `just test-affected` — Expected: PASS, including `//crates/clax-server:integration__1..6` and `//web:e2e_rest`.
- [ ] **Step 7: Commit**

```bash
git add crates/clax-core/src/events.rs crates/clax-server crates/clax-realtime web/e2e
git -c commit.gpgsign=false commit -m "Approve an artifact's declaration per version, and serve only what is approved"
```

### Task 4 (D4): The shell serves what is in force, and the approval bar

**Files:**
- Modify: `web/shell/src/api.ts:6` (artifact type: `capabilities_in_force?: Record<string, unknown>; may_approve?: boolean; pending_capabilities?: PendingCapabilities | null`)
- Modify: `web/shell/src/view/artifact-controller.ts:718` (`declared: (data.artifact.capabilities_in_force ?? {}) as Declared`); an `approve()` method (PUT, then refetch and reload the frame as `reload` does); `notNow()` hiding the bar for this view
- Modify: `web/shell/src/view/artifact-stream.ts:190-205` (`permissions` event → refetch the artifact)
- Create: `web/shell/src/caps/approval.ts` (copy and labels), `web/shell/src/ui/ApprovalBar.svelte` (bar and Review dialog), mounted in `web/shell/src/ui/StageIsland.svelte` beside `SpectatorBanner`
- Create: `web/e2e/permissions.spec.ts`
- Test: `web/shell/src/caps/approval.test.ts`, `web/shell/src/view/artifact-controller.test.ts`

**Interfaces:**
- Consumes: D3's artifact view fields and routes.
- Produces:
  - `export type PendingCapabilities = { version: number; capabilities: string[]; hosts: string[] }` (`api.ts`).
  - `export function pendingItems(p: PendingCapabilities): { name: string; label: string; detail: string }[]` and `export function barText(p: PendingCapabilities): string` (`caps/approval.ts`).
  - `export function approveCapabilities(id: string, version: number): Promise<void>` (`caps/approval.ts`, PUT with the token).

- [ ] **Step 1: Write failing unit tests** in `approval.test.ts`:

```ts
test("the bar names the version and what it asks for", () => {
  expect(barText({ version: 4, capabilities: ["net", "sample"], hosts: ["api.example.com"] }))
    .toBe("v4 asks for: the internet (api.example.com), Claude");
  // New hosts on an approved net, and a first declaration.
  expect(barText({ version: 2, capabilities: [], hosts: ["b.example.com"] })).toBe("v2 asks for: the internet (b.example.com)");
  expect(barText({ version: 1, capabilities: ["shell", "account"], hosts: [] })).toBe("v1 asks for: Clax's controls, sign-in");
});
test("each item says what approving lets the page do", () => {
  expect(pendingItems({ version: 1, capabilities: ["sample"], hosts: [] })[0].detail)
    .toBe("Ask Claude, paid for by this machine's Anthropic API key, within a daily spending cap.");
});
```

  and in `artifact-controller.test.ts`: `the_page_gets_the_declaration_in_force` (data with `capabilities: {net:…, db:{}}`, `capabilities_in_force: {db:{}}` → the host's `declared` is `{db:{}}`; without `capabilities_in_force` it is `{}`).
- [ ] **Step 2: Write the failing e2e tests** in `permissions.spec.ts` (page: `window.r = { net: (await claude.use("net")) !== null, db: (await claude.use("db")) !== null, state: await (await claude.use("permissions")).state("net") }`):
  - `an unapproved capability resolves null and appears once the owner approves`: publish `{net:{hosts:["api.clax.invalid"]}, db:{}}`; `r` is `{net:false, db:true, state:"unavailable"}`; the bar reads `v1 asks for: the internet (api.clax.invalid)`; Review → dialog `Let “<title>” use these?` → Approve; the frame reloads; `r.net === true`; `net.fetch("https://api.clax.invalid/x")` rejects `upstream_error` and no dialog appears.
  - `a version that adds a host asks again and the approved host keeps working`: approve v1, publish v2 adding `api2.clax.invalid`; bar `v2 asks for: the internet (api2.clax.invalid)`; fetch to `api.clax.invalid` → `upstream_error`, to `api2.clax.invalid` → `not_declared`.
  - `only the owner sees the bar`: a connected viewer's browser (existing people helpers) shows no bar.
- [ ] **Step 3: Run** `bazel test //web:unit` and `bazel test //web:e2e_rest --test_arg=e2e/permissions.spec.ts` — Expected: FAIL.
- [ ] **Step 4: Implement** `approval.ts`, `ApprovalBar.svelte`, the controller and stream changes.
- [ ] **Step 5: Run** `bazel test //web:unit //web:typecheck //web:lint //web:e2e_rest` — Expected: PASS.
- [ ] **Step 6: Commit**

```bash
git add web/shell web/e2e/permissions.spec.ts
git -c commit.gpgsign=false commit -m "Hand a page only its approved capabilities, and ask the owner to approve what a version adds"
```

### Task 5 (D5): The Permissions panel

**Files:**
- Create: `web/shell/src/ui/permissions-model.ts` (rows from the route's answer), `web/shell/src/ui/PermissionsPanel.svelte`
- Modify: `web/shell/src/ui/MoreMenu.svelte:28,87-90` (`onPermissions?: (() => void) | null`, a **Permissions…** item at every width), `web/shell/src/ui/more-menu.svelte.ts` (the panel's lazy load and opener), `web/shell/src/ui/TopbarIsland.svelte` (passes the opener when `may_approve`)
- Modify: `web/shell/src/caps/grants.ts` (`revoke(name: string): void` clears a stored grant for this viewer)
- Test: `web/shell/src/ui/permissions-model.test.ts`, `web/shell/src/caps/grants.test.ts`, `web/e2e/permissions.spec.ts`

**Interfaces:**
- Consumes: D3's `GET /api/artifacts/<id>/permissions`, the existing `DELETE net-grants`, `DELETE sample-grant`, `DELETE db-grants/<source>`, D3's `DELETE capability-approval/<name>`; D4's `pendingItems`.
- Produces: `export type PermissionRow = { key: string; label: string; state: string; revoke: { method: "DELETE"; path: string } | "local" | null }` and `export function permissionRows(p: PermissionsView, comments: PermissionState | null): PermissionRow[]`.

- [ ] **Step 1: Write failing unit tests** in `permissions-model.test.ts`:
  - `rows_list_each_grant_with_its_revoke`: an answer with net granted (`api.example.com`), sample granted, one db read of "Budget", `shell` approved at v3, comments `"granted"` → rows labelled `The internet (api.example.com)`, `Claude`, `Budget's data`, `Clax's controls`, `Comments by the page`, with revokes `DELETE /api/artifacts/<id>/net-grants`, `…/sample-grant`, `…/db-grants/<source>`, `…/capability-approval/shell`, `"local"`; states `Allowed since <date>` and `Approved for v3`.
  - `a_declared_host_not_granted_asks_on_first_use`: net declared, not granted → state `Asks on first use`, revoke `null`.
  - `pending_items_come_first`: a `pending` answer puts a `Waiting for approval` row first.
  - in `grants.test.ts`: `revoke_clears_a_stored_grant` (granted `comments`, `revoke("comments")` → `state("comments") === "prompt"`).
- [ ] **Step 2: Write the failing e2e test** `the Permissions panel revokes the internet and the page asks again` in `permissions.spec.ts`: approve via the bar; more menu → **Permissions…** → the internet row → Revoke; the next `net.fetch` opens the existing dialog `Let “<title>” use the internet?`.
- [ ] **Step 3: Run** `bazel test //web:unit` — Expected: FAIL.
- [ ] **Step 4: Implement** the model, the panel (each Revoke calls its route, then reloads the panel's answer) and the menu item.
- [ ] **Step 5: Run** `bazel test //web:unit //web:typecheck //web:lint //web:e2e_rest` — Expected: PASS.
- [ ] **Step 6: Commit**

```bash
git add web/shell web/e2e/permissions.spec.ts
git -c commit.gpgsign=false commit -m "List what a page may use in one Permissions panel, each with its revoke"
```

### Task 6 (D6): Agents learn what waits, and the contract says it

**Files:**
- Modify: `crates/clax-mcp/src/tools.rs` (the `publish` result: a line `Waiting for the owner's approval in Clax: the internet (api.example.com), Claude.` when the answer's `pending_capabilities` is set), `crates/clax-mcp/tests/tools.rs`
- Modify: `docs/contract.md` ("## Runtime capabilities": the approval paragraph below; "Fetching from the network": `not_approved`; "## Security model": who approves; the route table under "### The route table": the three routes, Denied to agents), the four `plugins/*/skills/clax/SKILL.md` (the same paragraph), `apps/clax_app/assets/prompt/runtime-capabilities.md` via `scripts/sync-app-prompt.sh`
- Modify: `docs/usage.md` (the Permissions panel and the approval bar, one paragraph)

**Interfaces:**
- Consumes: D3's `pending_capabilities` in publish answers.

The paragraph, word for word in the contract and the four skills:

> A capability that reaches past the page (`net`, `sample`, `account`, `shell`, `notify`, `share`, `file`, `camera`, `location`) is in force only once the artifact's owner approves the version that first declares it, or a new `net` host, in Clax. Until then `use()` resolves `null` for it, and the version before keeps what was approved. Publishing never approves, whoever publishes; the publish result says what waits. Tell the person to approve it in Clax.

- [ ] **Step 1: Write the failing MCP test** `publish_says_what_waits_for_approval` in `crates/clax-mcp/tests/tools.rs`: publishing `{net:{hosts:["api.clax.invalid"]}}` returns text containing `Waiting for the owner's approval in Clax: the internet (api.clax.invalid).`; a publish declaring `{db:{}}` has no such line.
- [ ] **Step 2: Run** `just test-affected` — Expected: FAIL.
- [ ] **Step 3: Implement** the line (labels as D4's `barText`, kept in `clax_core::capabilities` as `pub fn describe(p: &Added) -> String` so MCP and future CLI output share them), then edit the docs and run `scripts/sync-app-prompt.sh`.
- [ ] **Step 4: Run** `just test-affected` (includes `scripts/test-plugins.sh` and `//apps/clax_app:prompt_sync`) — Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/clax-core/src/capabilities.rs crates/clax-mcp docs plugins apps/clax_app/assets/prompt
git -c commit.gpgsign=false commit -m "Tell agents which capabilities wait for the owner's approval, and document approvals"
```

### Task 7 (D7): The Clax app approves what its person accepts

**Files:**
- Modify: `apps/clax_app/lib/clax/api.dart` (`Future<void> approveCapabilities(String aid, int version);`), `apps/clax_app/lib/clax/http_api.dart` (PUT `/api/artifacts/<aid>/capability-approval`, no session: the person's call), `apps/clax_app/lib/clax/fake_api.dart` (records calls in `approved: List<(String, int)>`)
- Modify: `apps/clax_app/lib/bundle/installer.dart:266` (after `createArtifact`), `apps/clax_app/lib/bundle/updater.dart:465,990` (after a publish whose `confirm` the person answered yes)
- Test: `apps/clax_app/test/bundle/installer_test.dart`, `apps/clax_app/test/bundle/updater_test.dart`, `apps/clax_app/test/clax/http_api_test.dart`

**Interfaces:**
- Consumes: D3's `PUT /api/artifacts/<id>/capability-approval {version}`.

- [ ] **Step 1: Write failing Dart tests**: `an install approves the version its person accepted` (fake API's `approved == [(aid, 1)]`); `an accepted update approves its version` (`[(aid, 2)]`); `an automatic update approves nothing` (`approved` empty); `approveCapabilities sends PUT with the version` (http_api_test: method `PUT`, path `/api/artifacts/a1/capability-approval`, body `{"version":3}`, no `X-Clax-Session`).
- [ ] **Step 2: Run** `bazel test //apps/clax_app:bundle_installer_test //apps/clax_app:bundle_updater_test //apps/clax_app:clax_http_api_test` — Expected: FAIL.
- [ ] **Step 3: Implement**; an approval refused (409 `stale_version`, network) is logged and leaves the install or update landed: the shell's bar asks instead.
- [ ] **Step 4: Run** the same command plus `//apps/clax_app:dart_analyze` — Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add apps/clax_app
git -c commit.gpgsign=false commit -m "Approve an installed or updated app's capabilities when its person accepts them"
```

---

## Before merge

- [ ] `just check` once; a lane failing in code this plan did not touch is rerun alone and both results reported.
- [ ] Qualifier records (one `qualifier record --stdin` batch, dry-run first): an `alternative` on `crates/clax-core/src/capabilities.rs` (`pending`) that `account.origins` widening is not yet an addition (`revisit:E`); an `alternative` on `crates/clax-server/src/routes/permissions.rs` that a push to a public Clax does not carry the local approval (`revisit:hosted-push`); a comment on `3e8b46b7` that the Rust port inherits its `db.read` / `db.share.read` blind spot. Close the spec's Q4 record `aafa9274` only with the owner's word; it is theirs.
- [ ] `qualifier diff main`.

## Self-review notes

- Spec §7 coverage: migration 57 and per-version approval (D2), the ported rule (D1), serving the last approved declaration (D2 `in_force`, D3 stamping, D4 host), the bar and its copy (D4), first declarations ask (D1 `pending(None, …)`, D2 test), the panel with net/sample/db-read/account/shell/comments rows and revokes (D5), the app's inspector untouched but feeding approvals (D7).
- E and F consume: `GATED` already lists `account` and `shell`; their consents are D's approvals (the panel's "approved for v<n>" rows); E adds `account.origins` to `added` when it lands.
