# No God Account Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On a hosted Clax, no credential (the owner's browser, `CLAX_TOKEN`, the owner's agents, any person) reads artifacts other people made unless they shared them or their link does; revoking a person takes their pages down until reinstated.

**Architecture:** One store-level answer, `Store::reach(aid, person)`, says whether an artifact is the caller's or the owner's (`Within`), someone else's (`Beyond`), or gone (missing, or its maker revoked). `clax_http::hosted::MemberRights` becomes the scope of every hosted caller, the owner included: granted rights apply `Within`, share rights and the link `Beyond`, nothing at `Gone`. The gate and `Agent<N>` attach it to every owner credential on hosted; then each route that decides by "owner" or "holds the daemon token" is swept to use it.

**Tech Stack:** Rust (axum, rusqlite), Svelte shell (`web/shell`), Bazel (`just test-affected`).

**Spec:** `docs/superpowers/specs/2026-10-22-no-god-account-design.md`

## Global Constraints

- Hosted only: with `s.hosted == None` (a local daemon) every behaviour stays as on `main`; every new check is behind `s.hosted.is_some()` or lives in hosted-only code.
- Out of reach answers 404 `not_found`, never 403: a refusal must not confirm an artifact exists (spec §6).
- No fallback, flag or break-glass that restores universal reading (spec §6).
- No migration: new state lives in the existing `install` table (`crates/clax-store/src/migrations.rs:545`).
- No `unsafe`; tests use the `ManualClock`, no sleeps; run tests with `just test-affected` (Bazel), never `cargo test`.
- `docs/contract.md` changes in the same task as the behaviour it describes.
- Write "ID" in prose. Doc comments describe the contract, not this plan.
- Commit with `git -c commit.gpgsign=false commit`, message ending `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Before editing a file, `qualifier threads <path>` (the plugin's `ensure-qualifier.sh exec threads <path>`).

## Review Focus

1. A daemon-token request on hosted from the machine (`localhost` host, the CLI or an MCP client over `/mcp`) reads a member's artifact: must be 404 like the owner's browser (Task 2, `the_daemon_token_on_hosted_reaches_only_the_owners_reach`).
2. A share made, then its maker revoked: the sharee must lose it (404), and get it back on reinstating (Task 1 store test, Task 8 HTTP test).
3. The owner's gallery on hosted still updates live for the owner's own artifacts but carries no event of a member's (Task 5, `the_gallery_topic_carries_only_what_its_subscriber_reads`).
4. A content address (`/c/…`, `/_blob/…`, `/_clip/…`) computed from `CLAX_TOKEN` alone answers 404 (Task 3, `a_content_key_is_not_derivable_from_the_daemon_token`).
5. An agent grant at `*` of a member (or of the owner's row) acting on another member's artifact shared with that member: allowed only where both the grant and the share allow (Task 2, `a_grant_at_any_reaches_another_persons_artifact_only_through_its_share`).

---

## Threads

- Closes b79ada9c ("A blocked GitHub account cannot be unblocked", `crates/clax-store/src/github.rs:355`): Task 8 (reinstating unblocks).
- Closes 82f069dd ("A push straight to Cloudflare Artifacts is credited to the owner and passes the rights recheck as the owner", `crates/clax-store/src/pushes.rs:488`) only in part: Task 6 makes the owner's recheck scoped; the attribution half stays open — reply on the thread, do not close.
- Touches 75c0506b ("A person whose rights do not read every artifact gets no gallery stream topic"): Task 5 closes it (scoped gallery channel).
- Touches 63ce5763 (a person's attention/seen without `?artifact=` and inbox items not filtered): Task 6 filters inbox items by reach; reply with what remains.
- Touches 4755b796 (rmcp `/mcp` acts with the daemon token): after Task 2 the daemon token is scoped on hosted; reply, do not close (local daemon unchanged).
- Not done here: ad4d8d8c (content keys never expire): Task 3 rotates keys once but keeps them unexpiring.

---

### Task 1: Reach and takedown in the store

**Files:**
- Create: `crates/clax-store/src/reach.rs`
- Modify: `crates/clax-store/src/lib.rs` (module), `crates/clax-store/src/shares.rs` (`share_rights`, `public_artifacts`), `crates/clax-store/src/hosted.rs` (`link_access`)
- Test: `crates/clax-store/src/reach.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub enum Reach { Gone, Within, Beyond }` (`#[derive(Clone, Copy, Debug, PartialEq, Eq)]`) re-exported as `clax_store::reach::Reach`.
  - `Store::reach(&self, aid: &str, person: Option<&str>) -> Result<Reach>`: `Gone` when no live artifact `aid` or its `artifacts.person` names a viewer with `person_revoked_at IS NOT NULL`; `Within` when `artifacts.person IS NULL` or equals `person`; else `Beyond`. `person: None` is the owner.
  - `Store::share_rights_of(&self, public_id: &str) -> Result<Rights>`: the rights of `public_id`'s shares alone (the owner's public ID included), leaving out artifacts whose maker is revoked.
  - `Store::link_access(aid)` returns `None` for an artifact whose maker is revoked; `Store::public_artifacts` leaves them out; `shares::share_rights` leaves them out.

- [ ] **Step 1: Write the failing tests** in `reach.rs`:
  - `reach_is_within_for_the_owners_and_ones_own_and_beyond_for_others`: owner artifact (`person NULL`) → `Within` for `None` and for `Some(mia)`; Mia's artifact → `Within` for `Some(mia)`, `Beyond` for `None` and `Some(sam)`; unknown ID → `Gone`.
  - `a_revoked_makers_artifacts_are_gone_to_everyone_until_reinstated`: revoke Mia (`Store::revoke_person`) → `reach(mias, Some(mia))`, `reach(mias, None)` both `Gone`; `link_access(mias)` `None` though a link was set; `share_rights_of(sam)` no longer allows read at it; `public_artifacts()` lacks it. (Reinstating is Task 8; assert the reinstated half there.)
  - `share_rights_of_holds_shares_alone`: Sam with stored `clax:collaborator` and one share of Mia's artifact → `share_rights_of(sam)` allows read at Mia's artifact and not at the owner's.
- [ ] **Step 2: Run** `just test-affected` (or `bazel test //crates/clax-store:…` the target it lists); expect the three to fail to compile.
- [ ] **Step 3: Implement** `Reach`, `Store::reach` (one indexed query joining `artifacts` to `viewers` on `public_id`), `share_rights_of` (wraps `shares::share_rights` by public ID), and the revoked-maker filter in `share_rights`, `link_access`, `public_artifacts`.
- [ ] **Step 4: Run** the store tests; expect pass, and no other store test changed.
- [ ] **Step 5: Commit** "Answer whether an artifact is in a person's reach, and take a revoked maker's pages down".

### Task 2: Every hosted caller is scoped

**Files:**
- Modify: `crates/clax-http/src/hosted.rs` (`MemberRights` ~:832, `of_session` ~:885, `reads_comments` ~:908, `session_reads` ~:926, `member_gate` ~:961, `gate` ~:1471, module docs), `crates/clax-http/src/agent.rs` (`Agent::from_request_parts` ~:1150, `read_rights` ~:806, `ReadRights` ~:870, `allowed_at` ~:1003), `docs/contract.md` (hosted "Security model", §8.5 owner reads)
- Test: `crates/clax-server/tests/api_hosted.rs`

**Interfaces:**
- Consumes: `Store::reach`, `Reach`, `Store::share_rights_of` (Task 1).
- Produces:
  - `pub struct MemberRights { pub person: Option<String>, pub rights: Arc<Rights>, pub shared: Arc<Rights> }` (`person: None` is the owner; `rights` the person's rights as today, `Rights::full()` for the owner; `shared` their share rights).
  - `MemberRights::of_person(st: &Store, public_id: &str) -> clax_core::Result<MemberRights>` and `MemberRights::owner(st: &Store) -> clax_core::Result<MemberRights>`.
  - `MemberRights::allows_at` rule: `Gone` → false; `Within` → `rights.allows(..., own = creator == person)`; `Beyond` → `shared.allows(...)`; then the link (unchanged).
  - `HostedRequest.member` is `Some` for the owner's session (cookie and shell-token paths) and for any request carrying the daemon token on a hosted daemon.
  - `Agent::member()` returns the hosted scope for every caller kind that carries one.
  - `ReadRights` applies the reach rule on hosted: `allows(a)` uses `rights` when `a.authors.creator` is `None` or the person, else `shared`.
  - `allowed_at` on hosted: `Gone` → false; `Beyond` → the grant's rights and its person's `share_rights_of` must both allow.
- [ ] **Step 1: Add test helper** `Site::made_by(&self, b: &Browser, title: &str) -> String` (POST `/api/artifacts:new {title, brief: "x"}` as `b`, returns `artifact.id`).
- [ ] **Step 2: Write the failing tests**:
  - Rewrite `a_persons_rights_limit_their_browser_and_the_owner_sees_everything` as `a_persons_rights_limit_their_browser_and_the_owner_reads_only_their_reach`: add `mias = site.made_by(&mia, "Mia's")`; owner listing = `[plan, budget]` (not `mias`); every path of the existing 404 loop answers the owner 404 at `mias`; after Mia shares `mias` with the owner as `viewer` (`POST /api/hosted/artifacts/{mias}/shares`, owner's public ID), the owner reads it (200) and still may not thread on it beyond viewer access (`thread` → 403 `not_permitted`).
  - `the_daemon_token_on_hosted_reaches_only_the_owners_reach`: with `site.ts` token (`Authorization: Bearer`, host `localhost`), `GET /api/artifacts` lacks `mias`; `GET /api/artifacts/{mias}` → 404; `GET /api/artifacts/{plan}` → 200.
  - `a_person_granted_any_reads_the_owners_and_their_own_alone`: Sam with `clax:collaborator` (`site.person`) lists `[plan, budget, sams]`, 404 at `mias`.
  - `a_grant_at_any_reaches_another_persons_artifact_only_through_its_share`: Sam mints an agent token with rights `*` (existing token-minting helper in `a_member_mints_agent_tokens_with_their_rights_and_revokes_them`); the token reads `mias` → 404; Mia shares `mias` with Sam as `commenter`; token reads 200, resolves a thread → 403 `not_permitted`.
  - `the_daemon_wide_event_stream_is_no_one_s_on_hosted`: `GET /api/events` → 403 for the owner's browser and for the daemon token.
- [ ] **Step 3: Run**; expect these to fail (owner reads `mias`).
- [ ] **Step 4: Implement** the `MemberRights` change and constructors; the gate attaches `MemberRights::owner` for the owner's session on both the cookie and the shell-token paths and for `has_token` requests, and runs `member_gate` for them; `member_gate` refuses `/api/events` to all; `Agent<N>` reads `HostedRequest.member` for every caller; `ReadRights` and `allowed_at` take the reach rule when `s.hosted` is set; `session_reads`/`reads_comments` drop the owner short-circuit. Drop the gallery topic for every hosted caller in `member_gate`'s `StreamBody` arm until Task 5 (the `every` test never holds now).
- [ ] **Step 5: Run** `just test-affected`; fix the existing hosted tests whose expectations assumed the owner or a `*` collaborator reads a member's artifact by changing their expectations to the reach rule (list each changed assertion in the commit message body). Local-daemon tests must pass unchanged.
- [ ] **Step 6: Update `docs/contract.md`**: hosted "Security model" and every sentence saying the owner reads every artifact on hosted (`grep -n "every artifact" docs/contract.md`).
- [ ] **Step 7: Commit** "Scope every hosted caller, the owner included, to their reach".

### Task 3: Content keys from a secret no credential reveals

**Files:**
- Modify: `crates/clax-store/src/hosted.rs` (new fn), `crates/clax-http/src/hosted.rs` (`content_key`, `asset_key`, `clip_key`, `asset_url`, `clip_link`, `stamp_content_keys`, `serve_content` ~:720-770, `Hosted` struct), `crates/clax-http/src/state.rs` (`AppState::assemble` ~:302), `crates/clax-server/src/shell/boot.rs:360`, `docs/contract.md` (content addresses)
- Test: `crates/clax-server/tests/api_hosted.rs`, unit tests in `crates/clax-http/src/hosted.rs`

**Interfaces:**
- Produces:
  - `Store::hosted_content_secret(&self) -> Result<String>`: the `install` row `hosted_content_secret`, made once (32 random bytes, lowercase hex) in one transaction.
  - `Hosted::content_secret(&self) -> &str` backed by a `OnceLock<String>`, set by `AppState::assemble` when `hosted` is `Some` (so the daemon and the Worker both set it).
  - `content_key`, `asset_key`, `clip_key` keep their signatures; every caller passes `h.content_secret()` instead of `&s.token`.
- [ ] **Step 1: Write the failing test** `a_content_key_is_not_derivable_from_the_daemon_token`: `clax_http::hosted::content_key(&site.ts.token, &plan, 1)` built into `/c/{plan}/v/1/k/{key}/index.html` → 404; the key the owner's `GET /api/artifacts/{plan}` stamps → 200.
- [ ] **Step 2: Run**; expect fail (the token-derived key serves).
- [ ] **Step 3: Implement** the secret, `Hosted::content_secret`, its setting in `assemble`, and the caller switch.
- [ ] **Step 4: Run** `just test-affected` (hosted content tests in `api_hosted_content.rs` and `api_hosted.rs` must pass; they read keys from responses).
- [ ] **Step 5: Contract**: content keys are keyed by the instance's own secret; old addresses stop working once.
- [ ] **Step 6: Commit** "Key hosted content addresses by a secret no credential reveals". Reply on ad4d8d8c that keys still never expire.

### Task 4: Only the maker administers and shares

**Files:**
- Modify: `crates/clax-http/src/agent.rs` (`administers` ~:953), `crates/clax-server/src/hosted.rs` (`may_share` :739), `crates/clax-store/src/shares.rs` (`already_owner` refusal), `crates/clax-store/src/elevations.rs` (owner's elevation locations), `docs/contract.md` (sharing, pin/capabilities/tags admin rule)
- Test: `crates/clax-server/tests/api_hosted.rs`

**Interfaces:**
- Produces: `administers(st, c, aid)` and `may_share(st, who, aid)` gain a hosted flag: `pub fn administers(st: &Store, c: &Caller, aid: &str, hosted: bool) -> Result<bool>`; on hosted the owner (and `OwnerAgent`, `PersonAgent{person: None}`) administers only `person IS NULL` artifacts. `may_share` on hosted: the maker alone. A share naming the owner is accepted on hosted.
- [ ] **Step 1: Failing tests** `only_the_maker_shares_pins_and_tags_a_page_on_hosted`: owner `PUT /api/hosted/artifacts/{mias}/link` → 404; `POST …/{mias}/shares` naming the owner → 404; owner `PATCH /api/artifacts/{mias} {pinned: true}` → 404; after Mia shares `mias` with the owner as `editor`, the owner's pin → 403 `admin_only`; Mia shares with the owner → 201 (not `already_owner`). And `an_owners_elevation_names_only_their_reach`: an owner elevation naming `mias` → 403 `not_creator`.
- [ ] **Step 2: Run**; fail. **Step 3: Implement**; update every caller of `administers` (pass `s.hosted.is_some()`). **Step 4: Run** `just test-affected`. **Step 5: Contract.** **Step 6: Commit** "On hosted, only an artifact's maker administers and shares it".

### Task 5: Streams carry only what their subscriber reads

**Files:**
- Modify: `crates/clax-realtime/src/stream.rs` (new channel and check, like `check_reads`/`reader_routes` ~:1015), `crates/clax-http/src/state.rs` (install the check, next to `stream.hide` ~:476), `crates/clax-http/src/hosted.rs` (`member_gate` `StreamBody`), `crates/clax-server/src/routes/stream.rs` (~:402-450: draft topics `!token && is_hidden`, `site`/`people`/`jobs` stay owner powers), `docs/contract.md` (stream topics)
- Test: `crates/clax-realtime/src/stream.rs` tests, `crates/clax-server/tests/api_hosted.rs`

**Interfaces:**
- Produces:
  - `Chan::GalleryOf(String)`: the gallery for one person key (`*owner` or a public ID); a hosted stream's `gallery` topic maps to it.
  - `Hub::check_gallery(&self, reads: impl Fn(&str /*person key*/, &str /*aid*/) -> bool + Send + Sync + 'static)`: asked without the hub's lock, per gallery event, per `GalleryOf` channel; set once.
  - `AppState::assemble` installs it on hosted with `MemberRights::{owner, of_person}(..).allows_at(st, (Artifacts, "read"), aid)`.
  - A draft (hidden) artifact's topics need it in the caller's reach on hosted, not only the token.
- [ ] **Step 1: Failing tests**: in `stream.rs`, `a_scoped_gallery_gets_only_events_its_check_admits` (two `GalleryOf` subscribers, check admits A for one only); in `api_hosted.rs`, `the_gallery_topic_carries_only_what_its_subscriber_reads` (owner subscribes `gallery`, topics answer `["gallery"]`; Mia publishes to `mias` → no event on the owner's stream before the owner's own `plan` update event arrives).
- [ ] **Step 2: Run**; fail. **Step 3: Implement**; re-admit `gallery` for every hosted caller in `member_gate`. **Step 4: Run** `just test-affected`. **Step 5: Contract.** **Step 6: Commit** "Scope the hosted gallery stream to each subscriber's reach". Resolve-proposal reply on 75c0506b with the test name.

### Task 6: Cross-artifact reads take the scope

**Files:**
- Modify: `crates/clax-server/src/routes/threads.rs` (`GET /api/threads` :315-363), `crates/clax-http/src/views/inbox.rs` (:30-156), `crates/clax-server/src/routes/people.rs` (`GET /api/agents` working entries :232-290), `crates/clax-server/src/routes/tags.rs` (listings, counts), `crates/clax-server/src/routes/toolpath.rs` (`owner_reads_history` :30, export), `crates/clax-http/src/db_caller.rs` (`base_caller` :201-233), `crates/clax-http/src/git_remotes.rs` (`Asker::Owner` :199, `allows` :268, `allows_one` :300), `crates/clax-server/src/routes/drafts.rs`, `crates/clax-server/src/routes/artifacts.rs` (`brief_refusal` :570), cross-app reads (`db_read_grants`, `/api/agent-read-consents`), `crates/clax-worker/src/diag.rs`, `docs/contract.md`
- Test: `crates/clax-server/tests/api_hosted.rs`

**Interfaces:**
- Consumes: `Agent::member()`, `MemberRights::allows_at`, `ReadRights` (Task 2).
- Produces: on hosted each path below filters or refuses by the caller's `MemberRights`; the local daemon is unchanged.
- [ ] **Step 1: Failing test** `no_owner_path_reads_a_members_page` — as the owner's browser and as the daemon token, with Mia's `mias` holding a thread, a db document, an asset, a draft and a tag:
  - `GET /api/threads` lists no thread of `mias`;
  - `GET /api/inbox` (and its groups) holds no item of `mias`;
  - `GET /api/agents` names no `working` entry at `mias` (an agent of Mia's working there);
  - `GET /api/tags` neither names nor counts `mias`'s tag;
  - `GET /api/toolpath/export` holds no event whose artifact is `mias` and still holds a `person.*` event;
  - `GET /api/artifacts/{mias}/docs/...` (db) → 404;
  - a git clone/token of `mias` (`POST /api/artifacts/{mias}/git-token` or the route `api_git_tokens.rs` uses) → 404;
  - `GET /api/artifacts/{mias}/drafts` → 404;
  - a cross-app read naming `mias` as source → 404.
- [ ] **Step 2: Run**; fail. **Step 3: Implement** path by path; each check is "`Agent::member()` (or the gate's `HostedRequest.member`) allows read at the artifact", filtering lists and answering 404 for named ones. `db_caller::base_caller` grants the token's level only `Within`. The Worker's `/_diag/repos/token` and `/_diag/artifacts/tree` require `Store::reach(aid, None) == Within`.
- [ ] **Step 4: Run** `just test-affected`. **Step 5: Contract** (threads listing, inbox, agents, tags, export, db levels, git remotes, drafts). **Step 6: Commit** "Scope the owner's cross-artifact reads on hosted". Reply on 82f069dd and 63ce5763 with what changed.

### Task 7: The remaining owner and token checks

**Files:**
- Modify: whatever the sweep finds; `docs/superpowers/specs/2026-10-22-no-god-account-design.md` is not edited.
- Test: `crates/clax-server/tests/api_hosted.rs`

- [ ] **Step 1: List** every site: `grep -rn 'is_owner()\|Caller::Owner\|OwnerAgent\|holds_token\|who\.token\|TokenOrAgent\|RequireToken\|owner_browser\|who\.owner' crates --include='*.rs' | grep -v /tests/` (about 320 lines on `main`).
- [ ] **Step 2: Classify** each in a scratch file outside the repository: (a) an owner power that names no artifact (People, invites, sessions, jobs, settings), (b) already scoped by Tasks 2–6, (c) reaches an artifact — fix. Every (c) gets a failing assertion added to `no_owner_path_reads_a_members_page` before its fix.
- [ ] **Step 3: Fix** each (c); **Step 4: Run** `just test-affected`. **Step 5:** For any (c) deliberately left, record a `concern` with `qualifier record` on its lines (detail: the route, what it reveals, why left). **Step 6: Commit** "Close the remaining owner bypasses on hosted".

### Task 8: People: new people only, reinstating, counts

**Files:**
- Modify: `crates/clax-store/src/people.rs` (`create_invite` :845, new `reinstate_person`, new `person_made`), `crates/clax-store/src/hosted.rs` (browser invite redemption ~:668-691), `crates/clax-store/src/github.rs` (unblock), `crates/clax-server/src/routes/people.rs` (`person_view` :491, new route), `crates/clax-server/src/router.rs` (:360), `crates/clax-server/src/routes/guards.rs` (route table), `web/shell/src/api.ts`, `web/shell/src/people/PersonCard.svelte`, `web/shell/src/people/model.ts`, `web/shell/src/people/fixtures.ts`, `docs/contract.md` (People)
- Test: store tests in `people.rs`, `crates/clax-server/tests/api_hosted.rs`, the People unit tests beside `web/shell/src/people/` (run `just web-test`)

**Interfaces:**
- Produces:
  - `Store::reinstate_person(&self, ctx: &AuditCtx, public_id: &str) -> Result<()>`: clears `person_revoked_at` and the GitHub block, records `person.reinstate`; `NotFound` for an unknown or unrevoked person; hands out nothing.
  - `Store::person_made(&self, public_id: &str) -> Result<Made>`, `pub struct Made { pub artifacts: u64, pub asset_bytes: u64 }` (live artifacts the person made; the summed `assets.size` of them).
  - `POST /api/people/{pid}/reinstate` (owner only, browser or token; agents 403 `not_for_agents`) → 200 the person's view.
  - `person_view` gains `"made": {"artifacts", "asset_bytes"}`; it names no artifact.
  - On hosted, `POST /api/people/invites` with `viewer` → 403 `new_people_only`; redeeming in a browser an invite that names an existing viewer row → 403 `new_people_only` (covers invites made before this change).
  - Shell: a revoked person's card shows "Reinstate" (`api.reinstatePerson(pid)`), and both cards show "N pages · X MB" from `made`.
- [ ] **Step 1: Failing tests**:
  - store `reinstating_restores_access_and_unblocks_github`: revoke a GitHub person, reinstate, `reach(theirs, Some(them))` → `Within`, the GitHub account signs in again.
  - HTTP `an_invite_on_hosted_makes_a_new_person_only`: owner invites `{viewer: mia_id}` → 403 `new_people_only`; and no other owner route signs a browser or machine in as Mia: the owner's sign-in-link and device-approval routes, called naming Mia, answer 403 or make a link for the owner alone (read `crates/clax-server/src/hosted.rs` sign-in links and `crates/clax-store/src/device.rs` to list them; assert each).
  - HTTP `revoking_takes_a_persons_pages_down_and_reinstating_brings_them_back`: Mia shares `mias` by link and with Sam; revoke Mia → anonymous `/a/{mias}` 404, Sam 404, its keyed content address 404, the public gallery lacks it; owner's `GET /api/people` shows Mia `made.artifacts == 1` and no title or ID of `mias` anywhere in the body (`!body.to_string().contains(&mias)`); reinstate → all 200 again.
  - web: PersonCard renders "Reinstate" for a revoked person and calls `reinstatePerson`.
- [ ] **Step 2: Run**; fail. **Step 3: Implement.** **Step 4: Run** `just test-affected` and `just web-test`. **Step 5: Contract** (People: invites, reinstating, takedown, `made`). **Step 6: Commit** "On hosted, invite new people only, reinstate revoked ones, and take their pages down while revoked". Then, in a separate commit, resolve b79ada9c with the test name and `--ref git:<sha>`.

### Task 9: Branch gate

- [ ] **Step 1:** `just check` (background, log to `target/check.log`); report the result as Bazel's.
- [ ] **Step 2:** `qualifier diff main`; reply on or re-record any drifted record on lines this branch changed.
- [ ] **Step 3:** Use `qual:closing-the-loop`, then `superpowers:finishing-a-development-branch`.
