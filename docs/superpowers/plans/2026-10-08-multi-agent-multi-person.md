# Multi-agent and Multi-person Clax Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Several agents of one person, and the agents of a second person on another machine, share one Clax: the owner connects people by invite or approved request; each machine holds a key-bound credential tied to its person; agents have their person's rights (narrowable by the person, extendable per agent by the owner); content written by anyone else reaches an agent through Clax's agent channels only after its person approves it, field by field, enforced by one daemon-side filter on every agent-facing read, write response and delivery path; agents have names, `@name` reaches exactly one, a sent thread has one owner agent with explicit handoff; ended sessions clean up; the inbox and roster are grouped by agent and say who is blocked on the person.

**Architecture:** A person is a `viewers` row (`person = 1`); an agent is a `sessions` row bound to a person and, from another machine, to a GNAP-shaped grant (migration 22), with registration, join and every session reference scoped to the caller's grant. Grants come from invites or from ad-hoc requests the owner approves (as new people) with the requester's user code; access tokens are opaque, hashed, key-bound (every request signed with the machine's Ed25519 key, `clax-ed25519`, verified before every handler), and live with their grant until revoked. Rights are GNAP access objects in `clax_core::rights`. Every handler learns its caller from one `Caller`; agent routes declare their right and filter class with a typed `Agent<N>` extractor that reads the handler's own `Path`. Visibility (migration 23: approvals, requests, document author sets) is one `clax_core::visibility::Visibility` per agent request, refreshed by a generation counter, applied per field to every response an agent channel gets and to every feedback tier. Migration 24 gives the inbox a recipient. The agent side gains a signed remote mode (`remote.json`) used by `clax connect`, the shim, hooks, the CLI and Pi. The shell gains a lazy People page, approval and connection cards, an agent strip and owner chips; the extension's panel gains approval cards.

**Tech Stack:** Rust 2024 (axum, tokio, rusqlite, serde, rmcp, clap, ring for Ed25519), Svelte 5 (runes) + TypeScript, Vite 6, Vitest + jsdom + @testing-library/svelte, Playwright (Chromium), Chrome MV3 extension, Pi extension (TypeScript, `node:crypto`).

**Spec:** `docs/superpowers/specs/2026-10-08-multi-agent-multi-person-design.md` revision 3 (with the main spec, the chrome-overlay spec and the questions-and-inbox spec). Read it in full before Task 1; each task names the sections it implements.

## Global Constraints

- Owner decisions O1–O9, rulings R1–R4 and design decisions M1–M27 are binding; a task that finds one unworkable stops and reports, it does not improvise.
- **Migration numbers.** Three migrations, written here as 22, 23 and 24 after main's 21. The `toolpath-audit` branch also claims 22: before Task 2 starts, check `MIGRATIONS.len()` on main; if the audit migration has landed, renumber these to follow it (with every comment, test name and spec reference). Never edit a migration that is on main.
- No `unsafe` Rust: every crate keeps `#![forbid(unsafe_code)]`. `ring` becomes a direct dependency of `clax-core` (already in `Cargo.lock`); no other new crypto crate.
- No hard links anywhere (`scripts/check-no-hard-links.sh` stays green); the remote clip cache writes files by copy.
- **Single-owner changes are exactly spec §19's eight.** Task 25's golden pins everything else; any other diff is a defect.
- **The filter is for agent channels only** (`Caller::OwnerAgent`, `Caller::PersonAgent`): people (owner browsers, the CLI used by hand, connected people's browsers, LAN viewers) see what they see today. **Every agent channel is an agent:** a token request naming a session in its path or `X-Clax-Session`, or carrying `X-Clax-Channel: agent`, is `OwnerAgent`; every agent-side client (shim, `/mcp`'s internal client, every `clax hook` command, `clax feedback follow`, Pi) sends the marker; the CLI's own commands never do.
- **Signatures are verified before every handler**, on every method, bodyless requests included.
- **One filter, per field.** Only `clax_core::visibility::Visibility` decides visibility, one field at a time. No route or store function re-implements the rule. `FeedbackItem` and `Notice` are built only in functions that take `&Visibility` (Task 12's guard).
- **One `Caller`.** No handler outside `identity.rs` and `auth.rs` calls `has_token` or reads `Authorization` (Task 6's guard).
- **Sessions are bound to their credential** (M4): every store function that matches or modifies a session takes the caller's `grant_id` (`None` for the token) and matches only rows with the same value.
- **Agent credentials never act as the owner**; M20's routes require `Caller::browser()`.
- **No unfiltered content to a person's agent at any commit:** from Task 5, every route classified `Filtered` answers `PersonAgent` callers 503 `not_yet` until the task that filters it lifts that for its routes (Tasks 9–12). Owner agents keep today's behaviour until then (no person row can exist before Task 4, and none is made outside tests before Task 20).
- Tokens, codes, keys, signatures and withheld content never appear in logs, errors, events or `Debug` output (`clax_core::Secret`, whose `Debug` prints `Secret(…)`).
- Untrusted text (person, agent and client names, comment bodies on approval cards): Svelte text interpolation only; the CLI escapes control and bidirectional characters; payloads quote with `feedback::quoted`.
- Doc comments and commit messages describe the contract or the change, never a conversation; in prose write "ID", never "id", except as a literal symbol.
- Tests: the whole `scripts/quality_gates.sh` stays within about 2 minutes on a warm cache; fake clocks (`ManualClock`), injected durations or paused Tokio time; wait on events, never a fixed sleep.
- Time to usable: `web/perf/bundle-budget.json` keeps `gallery` and `artifact` within +512 bytes, adds `people` at 16384, keeps `extPanel` at 65536; `scripts/perf-daemon-budget.json` adds `vis_threads_ms` and `vis_feedback_poll_ms` as the measured baseline plus 2 ms and 1 ms (Task 25), and `sig_verify_us: 150`; `web/perf/usable.perf.ts` stays green.
- UI changes are verified in a real browser (load the route, screenshots at 1280 px and 390 px, light and dark) before a UI task is reported done.
- Commit with `git -c commit.gpgsign=false commit …`; never `--no-gpg-sign`; never push or merge; never touch `~/.clax` (tests use temporary homes).

## Shared interfaces (defined once, used across tasks)

| Name | Where | Defined in | Shape |
|---|---|---|---|
| `Rights`, `RightType`, `Location`, `Access`, `effective`, presets | `clax_core::rights` | Task 1 | spec §7 |
| `Secret` | `clax_core::secret` | Task 2 | `pub struct Secret(String)`; `expose(&self) -> &str`; redacting `Debug` |
| `Clock` injected into `Store` (`Store::now()`), `TestServer::from_fixture_with_clock`, `golden::collect_agent_outputs` | `clax_core::store`, `clax_server::testing`, `clax-server/tests/golden` | Task 0 | the store's only time source |
| `RegisterSession { …, grant_id, repo, branch }`, `Store::register_session(r)` (time from the store's clock), `Store::join_session(j, grant_id: Option<&str>)`, `Store::list_sessions_for(grant_id: Option<&str>, live_only)`, `Store::session_of(sid, grant_id: Option<&str>) -> Result<Option<Session>>` | `clax_core::store::sessions` | Task 2 | M4 |
| `KeyPair`, `SignInput`, `SigHeader`, `parse_header`, `verify`, `NonceCache`, `SigError`, `fingerprint` | `clax_core::sig` | Task 3 | spec §6.6 |
| `Reader { Owner, Person(String) }` | `clax_core::visibility` | Task 5 (type only), Task 8 (the filter) | the person whose agent reads |
| `Caller` | `clax_server::auth` | Task 5 | (`OwnerAgent` for a token request naming a session or carrying `X-Clax-Channel: agent`) `Owner`, `OwnerAgent { session: Option<String> }`, `PersonAgent { person: Option<String>, grant: String, session: Option<String>, rights: Arc<Rights> }`, `Person { viewer: Viewer }`, `Viewer { viewer: Viewer }`, `Anonymous`; `reader() -> Option<Reader>`, `browser() -> bool`, `session() -> Option<&str>`, `is_agent() -> bool` |
| `Need` trait, `Agent<N: Need>` extractor, `Class { Filtered, Own, DeniedToGrants, GrantFlow }`, `need!` macro | `clax_server::agent` | Task 5 (extractor), Task 6 (every route) | M7 |
| `VisGen(Arc<AtomicU64>)` in `AppState` | `clax_server::state` | Task 8 | bumped by approval and rights changes |
| `Author`, `Visibility { for_reader, all, sees, withheld_authors, refresh_if_stale }`, author functions per field, `placeholder::*` | `clax_core::visibility` | Task 8 | spec §8 |
| `TakeResult { items, placeholders, touched }`, `Store::take_feedback(t, vis: &mut Visibility)` | `clax_core::store::feedback` | Task 12 | spec §8.4 |
| `World` (two people, canaries), `RealClients` (shim, `/mcp`, `clax hook`, `clax feedback follow`, Pi driven as processes against the test daemon) | `clax-server/tests/common/people.rs`, `clax-server/tests/common/clients.rs` | Task 6 (harness), extended by 9–12, 19 | spec §8.6 |

## Review Focus

1. **A session of one grant reached by another** (C1): Sam's shim registering or joining with the owner session's PID, or naming another grant's session in a path or header. Pinned in Task 2 (`remote_registration_never_matches_an_owner_session_with_the_same_pid`, `join_is_scoped_to_the_grant`) and Task 5 (`path_session_must_belong_to_the_grant`).
2. **A withheld field leaks** (C2, C3): a visible comment carrying Mia's anchor quote, clip or title; a write route returning the whole thread; any tier or notice. Pinned by Task 6's route guard and canary sweep (`no_canary_reaches_an_unapproved_agent`), and Tasks 9–12 (`visible_comment_on_withheld_page_hides_quote_and_title`).
3. **Documents launder authorship** (I7): Sam's agent patching one field of Mia's document; `where` used as an oracle. Pinned in Task 8 (`touching_a_field_never_takes_a_document`) and Task 11 (`where_runs_over_visible_documents_only`).
4. **A prompt-injected owner agent approves for itself** (I2): the token alone on decide, allow, revoke, rights, elevate, invite, grant approve. Pinned in Task 14 (`approval_changes_need_a_browser`) and Task 17 (`rights_changes_need_a_browser`).
5. **A sniffed request or token is reused** (M5): replay, stale timestamp, token without signature, a bad signature on a GET or DELETE, a forged token forcing a large body read. Pinned in Task 3 (`replay_and_staleness_are_refused`) and Task 5 (`authentication_precedes_the_body`, `a_bad_signature_is_refused_on_get_delete_and_post`).
8. **An owner agent channel classified as the owner** (N1): the hooks (path session), `clax feedback follow`, `/mcp`. Pinned in Task 5 (`every_agent_channel_is_an_agent`) and Task 12 (`hook_tiers_are_filtered_for_owner_agents`, `mcp_tools_are_filtered_for_the_owner`).
6. **A new machine becomes an existing person** (I4): approving a request into Sam or the owner. Pinned in Task 4 (`a_request_only_makes_a_new_person`).
7. **Single-owner behaviour changes beyond spec §19.** Pinned by Task 25 (`single_owner_outputs_are_unchanged`).

---

## File structure

```
crates/clax-core/src/rights.rs                  access objects, presets, intersect, union, allows
crates/clax-core/src/secret.rs                  Secret
crates/clax-core/src/sig.rs                     clax-ed25519: canonical string, sign, verify, NonceCache
crates/clax-core/src/names.rs                   agent name derivation, sanitising, reserved words, token form of person names
crates/clax-core/src/visibility.rs              Reader, Author, Visibility, per-field authors, placeholders
crates/clax-core/src/store/people.rs            person rows, invites, grants, tokens
crates/clax-core/src/store/approvals.rs         approvals, requests, release of held rows
crates/clax-core/src/store/migrations.rs        migrations 22, 23, 24
crates/clax-core/src/store/sessions.rs          grant binding, names, remote idle, end cleanup
crates/clax-core/src/store/feedback.rs          take_feedback under Visibility per field, placeholders, first take
crates/clax-core/src/store/threads.rs           owner, handoff, thread_taken, page_person
crates/clax-core/src/store/docs.rs              doc_authors, visible-only queries
crates/clax-core/src/store/artifacts.rs         artifacts/versions/assets person, by_page, page_person
crates/clax-core/src/store/inbox.rs             recipient, kinds approval/connection/ended
crates/clax-core/src/store/viewers.rs           fold follows every person/author column; name rules
crates/clax-server/src/auth.rs                  Caller (replaces RequireToken and inline token checks)
crates/clax-server/src/agent.rs                 Need, Agent<N>, need! declarations, pre-body authentication, GrantIndex
crates/clax-server/src/identity.rs              Caller resolution, Identity::person
crates/clax-server/src/routes/grants.rs         grant flow, invites, grant decisions
crates/clax-server/src/routes/people.rs         people, rights, elevate, narrow, agents roster, rename
crates/clax-server/src/routes/approvals.rs      approvals and requests
crates/clax-server/src/routes/connect.rs        GET /connect/b/<code>
crates/clax-server/src/routes/mod.rs            new routes; `mod agents` guard test
crates/clax-server/src/stream.rs                topics people, agents; person-scoped questions/inbox
crates/clax-server/src/extension.rs             gateway admits approval routes
crates/clax-server/tests/common/people.rs       World: two people, canaries, signed clients
crates/clax-server/tests/api_canary.rs          the canary sweep (HTTP and real clients)
crates/clax-server/tests/common/clients.rs      the real clients as processes
crates/clax-server/tests/golden_single_owner.rs the golden collector (Task 0) and comparison (Task 25)
crates/clax-mcp/src/client.rs, tools.rs         remote mode, status name, ask approval, comments_handoff, clip cache
crates/clax-cli/src/remote.rs                   remote.json, key, signing client
crates/clax-cli/src/commands/connect.rs         clax connect
crates/clax-cli/src/commands/people.rs          clax people, clax agents, clax approvals
plugins/pi/src/client.ts, remote.ts, clax.ts    remote mode, signing, new tool arguments
plugins/*/skills/clax/SKILL.md                  "Working beside other agents and people"
web/shell/src/people/                           People page, cards, agent strip
web/extension/src/panel/                        agent names; approval items link to the shell
web/e2e/people.spec.ts, visibility.spec.ts      end to end
scripts/perf-daemon.py, perf-daemon-budget.json baseline phase, two-person seed, budgets
docs/contract.md                                People, grants, rights, the route table, visibility, names, ownership
```

---

### Task 0: An injectable clock, the golden collector and the single-owner golden

Spec §19 (the golden test). This is the branch's first code commit and changes no behaviour, so the golden it writes is main's behaviour. Doing it here rather than on main first keeps main untouched while the branch is unmerged; the clock injection is behaviour-neutral by construction (the default clock is the system clock) and the existing suites prove it.

**Files:**
- Modify: `crates/clax-core/src/store/mod.rs` (`Store` holds an `Arc<dyn Clock>`, the `working::Clock` trait, defaulting to `SystemClock`; `Store::now()` reads it; `Store::open_with_clock`), `crates/clax-core/src/store/sessions.rs` (`register_session` and the reaper take time from the store's clock instead of `chrono::Utc::now()`), every other `chrono::Utc::now()` in `clax-core/src/store/` likewise, `crates/clax-server/src/testing.rs` (`TestServer::from_fixture_with_clock(sql, at)`)
- Create: `crates/clax-server/tests/golden_single_owner.rs` (`collect_agent_outputs(ts, tier)`, `normalise_minted(value)`, an ignored `write_golden` test), `crates/clax-server/tests/fixtures/single-owner-21.sql`, `crates/clax-server/tests/fixtures/single-owner-21.golden.json`

**Interfaces (produces):** `Store::open_with_clock(home, clock)`, `TestServer::from_fixture_with_clock`, `collect_agent_outputs(&TestServer, Tier) -> Value` (comments_read of every thread, read, list, versions, docs, roster, the tier's payload, notices, each through the real client for its channel where one exists: the hook binary for stop and prompt, `clax feedback follow` for notices), `normalise_minted(Value) -> Value` (keys `*_at`, `since`, `waited_s`, and ULIDs newer than the fixture, by an explicit list).

- [ ] **Step 1: Write the failing test** — `store_time_comes_from_the_injected_clock` (a `ManualClock` fixes `created_at`, `started_at` and `last_seen_at` of a registered session and a thread).
- [ ] **Step 2:** fails. **Step 3:** inject the clock; build the fixture (pre-migration-12 unattributed comments, anonymous and named LAN viewers, `via_page` comments and documents from the owner's browser, feedback in every state, questions, inbox, one live agent); write the collector; run `cargo test -p clax-server --test golden_single_owner -- --ignored --exact write_golden` to write the golden, one entry per tier, each on a fresh copy of the fixture under the fixed clock.
- [ ] **Step 4:** `scripts/quality_gates.sh` unchanged and green; running `write_golden` twice gives identical files.
- [ ] **Step 5: Commit** — `"Inject the store's clock, and record the single-owner agent-facing outputs as a golden"`

---

### Task 1: Rights

Spec §7.1–§7.3, M6.

**Files:** Create `crates/clax-core/src/rights.rs`; modify `crates/clax-core/src/lib.rs`.

**Interfaces (produces):**
- `pub enum RightType { Artifacts, Comments, Db, Ask }` (serde `clax-artifacts`, …)
- `pub enum Location { Any, Own, Artifact(String) }` (serde `"*"`, `"own"`, an ID)
- `pub struct Access { kind: RightType, actions: BTreeSet<String>, locations: Vec<Location>, level: Option<Level> }`
- `pub struct Rights(Vec<Access>)`: `parse(&Value) -> Result<Rights>` (arrays of objects and preset strings; 400 `invalid_rights`), `preset(&str)`, `full()`, `intersect`, `union`, `is_narrower_or_equal`, `allows(t, action, artifact: Option<&str>, own: &dyn Fn(&str) -> bool) -> bool`, `db_level(artifact, own) -> Option<Level>`, `to_json()`.
- `pub fn effective(person: &Rights, grant_narrow: Option<&Rights>, session_narrow: Option<&Rights>, elevate: Option<&Rights>) -> Rights`
- consts `PRESET_READER`, `PRESET_COLLABORATOR`, `PRESET_FULL`; action names per spec §7.1 (`clax-comments` has `read`, `write`, `reply`, `resolve`, `send`, `watch`).

- [ ] **Step 1: Write the failing tests** (bottom of `rights.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn own(a: &str) -> bool { a == "mine" }

    #[test]
    fn presets_parse_and_allow_what_the_spec_says() {
        let c = Rights::parse(&json!(["clax:collaborator"])).unwrap();
        assert!(c.allows(RightType::Artifacts, "read", Some("other"), &own));
        assert!(c.allows(RightType::Artifacts, "create", None, &own));
        assert!(c.allows(RightType::Artifacts, "update", Some("mine"), &own));
        assert!(!c.allows(RightType::Artifacts, "update", Some("other"), &own));
        assert!(!c.allows(RightType::Artifacts, "delete", Some("other"), &own));
        assert!(c.allows(RightType::Comments, "send", Some("other"), &own));
        assert_eq!(c.db_level("other", &own), Some(Level::Interact));
        assert_eq!(c.db_level("mine", &own), Some(Level::Admin));
        let r = Rights::preset(PRESET_READER).unwrap();
        assert!(!r.allows(RightType::Comments, "resolve", Some("x"), &own));
        assert!(!r.allows(RightType::Comments, "write", Some("x"), &own));
        assert!(Rights::full().allows(RightType::Artifacts, "delete", Some("x"), &own));
    }

    #[test]
    fn effective_is_person_narrowed_then_elevated() {
        let person = Rights::preset(PRESET_COLLABORATOR).unwrap();
        let no_delete = Rights::parse(&json!([
            {"type": "clax-artifacts", "actions": ["read", "create", "update"], "locations": ["*"]},
            {"type": "clax-comments", "actions": ["read", "write", "reply", "resolve", "send", "watch"], "locations": ["*"]},
            {"type": "clax-db", "actions": ["read", "write"], "level": "admin", "locations": ["*"]},
            {"type": "clax-ask", "actions": ["ask"]}])).unwrap();
        let up = Rights::parse(&json!([{"type": "clax-artifacts", "actions": ["update"], "locations": ["qr"]}])).unwrap();
        let e = effective(&person, Some(&no_delete), None, Some(&up));
        assert!(!e.allows(RightType::Artifacts, "delete", Some("mine"), &own), "narrowed away");
        assert!(e.allows(RightType::Artifacts, "update", Some("qr"), &own), "elevated past the person");
        assert!(!e.allows(RightType::Artifacts, "update", Some("other"), &own));
        assert_eq!(e.db_level("other", &own), Some(Level::Interact), "level takes the lower");
    }

    #[test]
    fn narrowing_is_checked_and_junk_is_refused() {
        let c = Rights::preset(PRESET_COLLABORATOR).unwrap();
        assert!(Rights::preset(PRESET_READER).unwrap().is_narrower_or_equal(&c));
        assert!(!Rights::full().is_narrower_or_equal(&c));
        for bad in [json!("clax:nope"), json!([{"type": "clax-live", "actions": ["read"]}]),
                    json!([{"type": "clax-artifacts", "actions": ["fly"]}]),
                    json!([{"type": "clax-db", "actions": ["write"], "level": "owner"}])] {
            assert!(matches!(Rights::parse(&bad), Err(crate::CoreError::Invalid { .. })), "{bad}");
        }
    }

    #[test]
    fn round_trips_as_gnap_access_objects() {
        let c = Rights::preset(PRESET_COLLABORATOR).unwrap();
        assert_eq!(Rights::parse(&c.to_json()).unwrap(), c);
    }
}
```

- [ ] **Step 2:** `cargo test -p clax-core rights::` fails (no module).
- [ ] **Step 3: Implement** per spec §7. Keep `Own ∩ Artifact(a)` as a conjunction evaluated in `allows` (do not resolve `own` early).
- [ ] **Step 4:** tests pass; `cargo clippy -p clax-core -- -D warnings`.
- [ ] **Step 5: Commit** — `"Add GNAP-shaped access rights with presets, narrowing and elevation"`

---

### Task 2: Migration 22, the people store, and session binding in the store

Spec §5.1, §6.2–§6.4, §6.7, M1–M5.

**Files:**
- Modify: `crates/clax-core/src/store/migrations.rs` (migration 22), `crates/clax-core/src/store/sessions.rs` (grant binding in `register_session`, `join_session`, listing; `session_of`), `crates/clax-core/src/model.rs` (`Viewer.person`, `Viewer.person_revoked_at`, `Session.{grant_id, person, name, name_source}`), `crates/clax-core/src/store/viewers.rs` (fold rewrites `sessions.person`, `artifacts.person`, `versions.person`, `versions.page_person`, `assets.person`), `crates/clax-core/src/store/artifacts.rs` and `assets.rs` (write `person`, `by_page`, `page_person`)
- Create: `crates/clax-core/src/store/people.rs`, `crates/clax-core/src/secret.rs`

**Interfaces (produces):**
- `Store::create_invite(who: InvitePerson, now) -> Result<(Invite, Secret)>`, `InvitePerson::{New(String), Viewer(String), Owner}`; `list_invites`, `revoke_invite`.
- `Store::request_grant(g: NewGrant, now) -> Result<GrantStart>`; `NewGrant { key: [u8; 32], client_name, peer_ip, access: Rights, invite: Option<String> }`; `GrantStart::{Approved { grant, token: Secret, manage: Secret, continue_token: Secret, browser_code: Secret }, Pending { grant, continue_token: Secret, user_code: Secret }}`; errors `invalid_invite`, `limit_reached`.
- `approve_grant(gid, user_code, name, rights: Option<Rights>, now)` (always a new person; `wrong_code`, third strike denies), `deny_grant`, `continue_grant(gid, continue_token, now) -> Result<ContinueOutcome>` (`too_fast`, `denied`), `revoke_token(gid, manage_token)`, `revoke_grant(gid, by) -> Result<RevokedGrant { sessions_ended }>`, `revoke_person(public_id) -> Result<RevokedPerson>`, `narrow_grant(gid, Rights)`.
- `token_index() -> Result<Vec<([u8; 32], String)>>`; `person_rights(public_id) -> Result<Rights>`; `set_person_rights`; `expire_grants(now) -> Result<Vec<String>>` (pending past 10 min; unused 30 days).
- Session binding (M4): `RegisterSession` gains `grant_id: Option<String>`, `repo`, `branch` (names in Task 7); every match in `register_session(r)` and `join_session(j, grant_id)` adds `AND grant_id IS ?` with the caller's value; `list_sessions_for(grant_id, live_only)`; `session_of(sid, grant_id)` returns `None` for another credential's session.

- [ ] **Step 1: Write the failing tests** (`store/people.rs` and `store/sessions.rs` tests; `now` arguments from a `ManualClock`)

```rust
#[test]
fn migration_22_from_21_keeps_rows_and_adds_owner_nulls() {
    let st = Store::open_at_version(tmp(), 21);
    seed_21(&st); // a session, an artifact with 2 versions and an asset, a LAN viewer
    st.migrate_to_latest().unwrap();
    assert!(st.list_sessions(false).unwrap().iter().all(|s| s.person.is_none() && s.grant_id.is_none() && s.name.is_none()));
    assert!(st.raw_artifact_people().unwrap().iter().all(Option::is_none));
    assert!(!st.get_viewer(SEEDED).unwrap().unwrap().person);
}

#[test]
fn remote_registration_never_matches_an_owner_session_with_the_same_pid() {
    let st = store();
    let owner = st.register_session(RegisterSession { harness: "claude".into(), parent_pid: Some(4242), cwd: "/o".into(), ..Default::default() }).unwrap();
    let (gid, _) = approved(&st, "Sam");
    let sam = st.register_session(RegisterSession { harness: "claude".into(), parent_pid: Some(4242), cwd: "/s".into(),
                                                    grant_id: Some(gid.clone()), ..Default::default() }).unwrap();
    assert_ne!(sam.id, owner.id);
    let o = st.get_session(&owner.id).unwrap().unwrap();
    assert_eq!((o.cwd.as_str(), o.grant_id.as_deref()), ("/o", None), "the owner's row is untouched");
    assert_eq!(sam.person.as_deref(), Some(SAM_PID));
}

#[test]
fn join_is_scoped_to_the_grant() {
    let st = store();
    let owner = st.register_session(reg_pid("codex", 777)).unwrap();
    let (gid, _) = approved(&st, "Sam");
    let j = st.join_session(join_pid("codex", 777, "hs-1"), Some(&gid)).unwrap();
    assert!(j.is_none() || j.as_ref().unwrap().id != owner.id);
    assert!(st.get_session(&owner.id).unwrap().unwrap().harness_session_id.is_none());
    assert!(st.session_of(&owner.id, Some(&gid)).unwrap().is_none());
    assert!(st.list_sessions_for(Some(&gid), true).unwrap().iter().all(|s| s.grant_id.as_deref() == Some(gid.as_str())));
}

#[test]
fn invite_is_single_use_and_binds_the_first_key() {
    let st = store();
    let (_inv, code) = st.create_invite(InvitePerson::New("Sam".into()), T0).unwrap();
    assert!(matches!(st.request_grant(grant(KEY_A, Some(code.expose())), T0).unwrap(), GrantStart::Approved { .. }));
    assert_invalid(st.request_grant(grant(KEY_B, Some(code.expose())), T0), "invalid_invite");
}

#[test]
fn a_request_only_makes_a_new_person() {
    let st = store();
    let GrantStart::Pending { grant, user_code, .. } = st.request_grant(grant(KEY_A, None), T0).unwrap() else { panic!() };
    let g = st.approve_grant(&grant.id, user_code.expose(), "Sam", None, T0).unwrap();
    let v = st.viewer_of_grant(&g.id).unwrap();
    assert!(v.person && !v.owner && v.display_name.as_deref() == Some("Sam"));
    // There is no API taking an existing viewer or the owner row for a request (type-level), and the name rules apply:
    let GrantStart::Pending { grant, user_code, .. } = st.request_grant(grant(KEY_B, None), T0).unwrap() else { panic!() };
    assert_invalid(st.approve_grant(&grant.id, user_code.expose(), "sam", None, T0), "name_taken");
}

#[test]
fn wrong_code_approves_nothing_and_the_third_denies() { /* two wrong_code leave it pending; the third denies */ }

#[test]
fn pending_limits_per_ip_and_overall() { /* 3 per peer IP, 20 in all: limit_reached */ }

#[test]
fn the_manage_token_revokes_and_the_access_token_cannot() {
    let st = store();
    let (gid, t1, manage) = approved_with_manage(&st);
    assert_invalid(st.revoke_token(&gid, t1.expose()), "invalid_token");
    st.revoke_token(&gid, manage.expose()).unwrap();
    assert!(!st.token_live(t1.expose()).unwrap());
}

#[test]
fn grants_expire_after_thirty_days_unused_and_tokens_never_alone() { /* ManualClock: +29 d live, +31 d expired */ }

#[test]
fn revoking_a_grant_ends_its_sessions_and_revoking_a_person_keeps_history() {}

#[test]
fn secrets_never_print() { assert!(!format!("{:?}", Secret("cxg_abc".into())).contains("abc")); }
```

- [ ] **Step 2:** `cargo test -p clax-core people:: sessions::` fails.
- [ ] **Step 3: Implement** migration 22 exactly as spec §5.1 (including `versions.file_authors_json`, written by Task 8), the store, and the binding. Codes from `getrandom`: invite 12 Crockford base32 characters, user code 4 letters (no I, O) and 4 digits, tokens `cxg_`/`cxm_`/`cxc_` + 32 bytes base64url, browser code 32 bytes; all stored as SHA-256 hex. Every existing caller of `register_session`/`join_session` passes `None` (the token) until Task 5.
- [ ] **Step 4:** `cargo test -p clax-core` (migration tests included) and `cargo test -p clax-server` (unchanged behaviour).
- [ ] **Step 5: Commit** — `"Add people, invites, grants and key-bound tokens, and bind sessions to their credential (migration 22)"`

---

### Task 3: `clax-ed25519` signatures and the nonce cache

Spec §6.6, M5.

**Files:** Create `crates/clax-core/src/sig.rs`; modify `crates/clax-core/Cargo.toml` (`ring = { workspace = true }`), the workspace `Cargo.toml` (`ring` at the locked version).

**Interfaces (produces):** `KeyPair` (`generate`, `from_pkcs8`, `pkcs8() -> Secret`-like bytes, `public() -> [u8; 32]`, `sign(&SignInput) -> SigHeader`); `SignInput<'a> { method, path_and_query, host, t: i64, nonce: String, body_sha256: [u8; 32], token_sha256: [u8; 32] }` with `canonical()`; `parse_header(&str) -> Result<SigHeader>`; `verify(public, input, sig) -> bool`; `NonceCache::new(window_s: i64 /* 240 */, per_key: usize /* 65_536 */)` with `check(&mut self, key, nonce, t, now) -> Result<(), SigError>` (checked before the body) and no separate insert (check inserts); `SigError::{Stale, Replayed, Full, Bad}`; `fingerprint(public) -> String`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn canonical_string_is_fixed() {
    let i = input("POST", "/api/artifacts?x=1", "192.168.1.5:7480", 1_760_000_000, "n0", b"{}", "cxg_t");
    let s = String::from_utf8(i.canonical()).unwrap();
    let lines: Vec<_> = s.lines().collect();
    assert_eq!(lines[..6], ["clax-ed25519 v1", "POST", "/api/artifacts?x=1", "192.168.1.5:7480", "1760000000", "n0"]);
    assert_eq!(lines[6], "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a");
}

#[test]
fn sign_verify_and_tamper() {
    let k = KeyPair::generate();
    let i = input("GET", "/api/artifacts", "h", 100, "n1", b"", "t");
    let h = k.sign(&i);
    assert!(verify(&k.public(), &i, &h.sig));
    for bad in [input("POST", "/api/artifacts", "h", 100, "n1", b"", "t"), input("GET", "/api/artifacts?a", "h", 100, "n1", b"", "t"),
                input("GET", "/api/artifacts", "x", 100, "n1", b"", "t"), input("GET", "/api/artifacts", "h", 100, "n1", b"x", "t"),
                input("GET", "/api/artifacts", "h", 100, "n1", b"", "u")] {
        assert!(!verify(&k.public(), &bad, &h.sig));
    }
    assert!(!verify(&KeyPair::generate().public(), &i, &h.sig));
}

#[test]
fn replay_and_staleness_are_refused() {
    let mut c = NonceCache::new(240, 4);
    let key = [7u8; 32];
    assert!(c.check(&key, "a", 1000, 1000).is_ok());
    assert_eq!(c.check(&key, "a", 1000, 1001), Err(SigError::Replayed));
    assert_eq!(c.check(&key, "b", 1000, 1121), Err(SigError::Stale));
    assert_eq!(c.check(&key, "b", 1000, 879), Err(SigError::Stale));
    for n in ["c", "d", "e"] { c.check(&key, n, 1000, 1000).unwrap(); }
    assert_eq!(c.check(&key, "f", 1000, 1000), Err(SigError::Full));
    assert!(c.check(&key, "f", 1300, 1300).is_ok(), "entries older than the window are evicted");
}

#[test]
fn pkcs8_round_trips_and_header_parses() {}
```

- [ ] **Steps 2–4:** fail, implement with `ring::signature::{Ed25519KeyPair, UnparsedPublicKey, ED25519}`, pass (`cargo test -p clax-core sig::`; the unsafe gate stays green).
- [ ] **Step 5: Commit** — `"Sign agent requests with Ed25519 over a fixed canonical string, with a nonce cache"`

---

### Task 4: Grant routes, invites, the browser link and the `people` topic

Spec §6.2–§6.5, §4.2, M20 (browser-only decisions).

**Files:**
- Create: `crates/clax-server/src/routes/grants.rs`, `crates/clax-server/src/routes/connect.rs`
- Modify: `crates/clax-server/src/routes/mod.rs` (routes; `l10::GUARDS` entries), `crates/clax-server/src/state.rs` (`GrantIndex`, `NonceCache` behind a mutex, the clock), `crates/clax-server/src/stream.rs` (`Topic::People`, owner streams only), `crates/clax-server/src/daemon.rs` (load the index; the expiry sweep every 60 s), `crates/clax-server/src/testing.rs` (`TestClient`: a key pair that signs; `ts.invite_command(name)`, `ts.connect_person(name, preset) -> TestClient`, `ts.owner_browser()` for browser-only routes)
- Test: `crates/clax-server/tests/api_grants.rs`

**Interfaces:** consumes Tasks 2–3; produces the grant-flow routes (`POST /api/grants`, `POST|DELETE /api/grants/{gid}/continue`, `DELETE /api/grants/{gid}/token`), `POST|GET /api/people/invites`, `DELETE /api/people/invites/{id}`, `GET /api/grants`, `POST /api/grants/{gid}/approve|deny`, `DELETE /api/grants/{gid}`, `GET /connect/b/{code}`; `GrantIndex { lookup(&[u8; 32]) -> Option<GrantRef>, reload(&Store) }`. Until Task 5's `Caller`, the browser-only check is `who.owner_browser()`; Task 5 switches it to `Caller::browser()`.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn invite_flow_issues_a_key_bound_token_and_a_browser_link() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let cmd = ts.invite_command("Sam").await; // through the owner's browser credential
    let c = TestClient::new(&ts);
    let r = c.grant_request(json!({"access_token": {"access": ["clax:collaborator"]}, "clax": {"invite": code_of(&cmd)}})).await;
    assert!(r["access_token"]["value"].as_str().unwrap().starts_with("cxg_"));
    assert!(r["access_token"].get("expires_in").is_none() && r["access_token"].get("flags").is_none());
    assert!(r["access_token"]["manage"]["access_token"]["value"].as_str().unwrap().starts_with("cxm_"));
    assert_eq!(r["clax"]["person"]["display_name"], "Sam");
    let link = r["clax"]["browser_link"].as_str().unwrap();
    let (lan, _) = ts.lan();
    let planted = lan.get(link).header("cookie", "clax_viewer=01J9Z3K4M5N6P7Q8R9S0T1V2W3").send().await.unwrap();
    assert!(planted.headers().get_all("set-cookie").iter().any(|v| v.to_str().unwrap().starts_with("clax_viewer=")));
    assert!(ts.viewer_exists("01J9Z3K4M5N6P7Q8R9S0T1V2W3").await, "the carried viewer is replaced, never folded");
    assert_eq!(lan.get(link).send().await.unwrap().status(), 404, "one use");
}

#[tokio::test]
async fn invites_need_the_lan_and_a_browser() {
    let ts = TestServer::spawn().await;
    assert_eq!(ts.owner_browser().post("/api/people/invites", json!({"name": "Sam"})).await.status(), 409);
    let lan = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let res = lan.post_json("/api/people/invites", json!({"name": "Sam"})).await; // token alone
    assert_eq!((res.status().as_u16(), code(res).await.as_str()), (403, "browser_required"));
}

#[tokio::test]
async fn adhoc_request_waits_for_the_owner_with_the_code() {
    /* request → 201 with user_code, continue.wait 5; the people topic announces it; approve with a wrong code → 400
       wrong_code; with the right code and a name → 200; after the clock passes 5 s, continue returns the token */
}

#[tokio::test]
async fn a_request_only_makes_a_new_person() {
    /* approve body naming {"viewer": "<Sam's public ID>"} or {"me": true} → 400 invalid_args; "name" equal to an
       existing person's name → 409 name_taken */
}

#[tokio::test]
async fn polling_faster_than_wait_is_too_fast_and_expiry_denies() {}

#[tokio::test]
async fn revocation_through_manage_and_continue() {
    /* DELETE token with the manage token: that access token is 401 invalid_token at once; DELETE continue → grant
       revoked, every token 401 grant_revoked; there is no POST on the token route (405) */
}

#[tokio::test]
async fn grant_routes_are_signed() {
    /* POST /api/grants without a signature → 401 invalid_signature; signed by key A presenting key B → 401 */
}
```

- [ ] **Steps 2–4:** fail, implement, pass (`cargo test -p clax-server --test api_grants`, then the whole crate).
- [ ] **Step 5: Commit** — `"Add the GNAP grant flow: invites, user-code requests for new people, continuation and revocation"`

---

### Task 5: `Caller`, the `Agent<N>` extractor, pre-body authentication and session binding at the routes

Spec §6.1, §6.6 (order of checks, verification before the handler), §6.7, M4, M7, M8.

**Files:**
- Create: `crates/clax-server/src/agent.rs`; `crates/clax-core/src/visibility.rs` with only `pub enum Reader { Owner, Person(String) }`
- Modify: `crates/clax-server/src/auth.rs` (`Caller`; `RequireToken` becomes a thin `Caller` check for routes that stay owner-only), `crates/clax-server/src/identity.rs` (`Identity::caller(&self, st, headers) -> Result<Caller>`, `Identity::person()`), `crates/clax-server/src/routes/sessions.rs` (register, join, list, get, patch take `Caller` and pass `grant_id`), `crates/clax-server/src/routes/mod.rs` (the pre-body authentication layer, outermost after the extension gateway), `crates/clax-server/src/routes/grants.rs` (browser checks via `Caller::browser()`)
- Test: `crates/clax-server/tests/api_agent_auth.rs`

**Interfaces (produces):**
- `Caller` (Shared interfaces table).
- `pub trait Need { const RIGHT: Option<(RightType, &'static str)>; const CLASS: Class; }`; `pub struct Agent<N: Need> { pub caller: Caller, pub artifact: Option<ArtifactId>, _n: PhantomData<N> }` implementing `FromRequestParts`: resolves `Caller`; for `PersonAgent` checks `N::RIGHT` at the artifact parsed from the route's own `Path` (`{aid}`), 403 `not_permitted` with `{"right": {"type", "action"}}`; checks every `{id}` session segment and `X-Clax-Session` against the caller (403 `foreign_session`); `Class::Filtered` answers `PersonAgent` 503 `not_yet` while the route's filter flag (`agent::FILTERED_LIVE`, a `&[&str]` of handler names Tasks 9–12 add to) does not list it.
- `agent::authenticate` middleware: for `Authorization: Clax-Grant`, checks token (`GrantIndex`), header shape, timestamp and nonce before reading the body; then verifies the signature before the handler runs: a bodyless request (no or zero `Content-Length`, no `Transfer-Encoding`) at once with the empty digest, any other by reading the body in full (capped by the route's limit), hashing and verifying it, and handing the buffered body on (401 `invalid_signature`). Refuses `Origin`/`Cookie` with an agent credential.
- `Caller` resolution for the token: `OwnerAgent { session }` when the path names a session (`/api/sessions/{id}/…`) or `X-Clax-Session` is present; `OwnerAgent { session: None }` with `X-Clax-Channel: agent`; otherwise `Owner`. The marker is ignored without the token.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn a_persons_agent_registers_under_its_grant_and_reaches_only_its_sessions() {
    let ts = lan_server().await;
    let owner_sid = ts.register_session_pid("claude", "h1", 4242).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register_pid("claude", "h1", 4242).await["id"].as_str().unwrap().to_string();
    assert_ne!(sid, owner_sid);
    let list: Value = sam.get(None, "/api/sessions?live=true").await.json().await.unwrap();
    assert!(list["sessions"].as_array().unwrap().iter().all(|s| s["id"] == sid.as_str()));
}

#[tokio::test]
async fn path_session_must_belong_to_the_grant() {
    let ts = lan_server().await;
    let owner_sid = ts.register_session("claude", "h1").await["id"].as_str().unwrap().to_string();
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register("claude", "h2").await["id"].as_str().unwrap().to_string();
    for p in [format!("/api/sessions/{owner_sid}/feedback"), format!("/api/sessions/{owner_sid}"),
              format!("/api/sessions/{owner_sid}/working"), format!("/api/sessions/{owner_sid}/watches")] {
        let r = sam.get(Some(&sid), &p).await;
        assert_eq!((r.status().as_u16(), code(r).await.as_str()), (403, "foreign_session"), "{p}");
    }
    let r = sam.get(Some(&owner_sid), "/api/artifacts").await; // header names a foreign session
    assert_eq!(r.status(), 403);
}

#[tokio::test]
async fn authentication_precedes_the_body() {
    /* a forged Clax-Grant token with a 20 MiB asset body → 401 invalid_token, and the server's body-read counter
       (a debug test route) shows nothing read; a valid token with a stale t → 401 before reading */
}

#[tokio::test]
async fn a_bad_signature_is_refused_on_get_delete_and_post() {
    let ts = lan_server().await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let sid = sam.register("claude", "h1").await["id"].as_str().unwrap().to_string();
    for (m, p) in [("GET", "/api/artifacts".to_string()), ("DELETE", format!("/api/sessions/{sid}/watches/{}", ts.any_artifact().await)),
                   ("POST", format!("/api/sessions/{sid}/feedback/ack"))] {
        let r = sam.call_with_bad_signature(&sid, m, &p).await; // valid token, fresh t and nonce, wrong s
        assert_eq!((r.status().as_u16(), code(r).await.as_str()), (401, "invalid_signature"), "{m} {p}");
    }
}

#[tokio::test]
async fn every_agent_channel_is_an_agent() {
    /* with the daemon token: GET /api/sessions/{sid}/feedback (no header) resolves OwnerAgent{sid}; a request with
       X-Clax-Channel: agent and no session resolves OwnerAgent{None}; the same without the marker is Owner; the marker
       with a viewer cookie and no token is ignored (a debug route echoes the resolved Caller) */
}

#[tokio::test]
async fn token_without_signature_is_refused_and_browsers_cannot_mix() {}

#[tokio::test]
async fn a_grant_is_never_the_owner() { /* the owner's own second machine: GET /api/inbox 403; sessions person NULL */ }

#[tokio::test]
async fn session_less_grant_requests_use_person_and_grant_rights() {
    /* sam.get(None, "/api/artifacts") → 503 not_yet (Filtered, not yet live) — and after Task 10 200; a session
       route without X-Clax-Session → 400 no_session */
}

#[tokio::test]
async fn token_callers_are_unchanged() { /* the existing api_* suites run unchanged */ }
```

- [ ] **Steps 2–4:** fail, implement, pass (`cargo test -p clax-server`).
- [ ] **Step 5: Commit** — `"Resolve every request to one Caller, authenticate agent credentials before the body, and bind session references to their grant"`

---

### Task 6: Classify every route, the route guard, the no-inline-credentials guard and the canary harness

Spec §7.5, §8.6, M7, M8; review C3, I8, I10, I13, M12.

**Files:**
- Modify: `crates/clax-server/src/routes/mcp.rs` and `crates/clax-mcp/src/client.rs` (`/mcp`'s internal client and every agent-side client send `X-Clax-Channel: agent`), `crates/clax-cli/src/client.rs` (hook commands and `feedback follow` send it; other commands never), every route module (`artifacts`, `assets`, `content`, `docs`, `threads`, `sessions`, `watches`, `working`, `feedback`, `questions`, `live`, `sites`, `inbox`, `viewers`, `events`, `stream`, `sample`, `room`, `extension`, `token`, `shell`, `health`, `grants`, `connect`) — each agent-reachable handler takes `Agent<N>` declared with `need!` per spec §7.5; every other handler takes `DeniedToGrants` (an extractor answering `PersonAgent` 403 `forbidden`) or `GrantFlow`; every inline `has_token(&headers, …)` becomes a `Caller` match (agent replies: `caller.is_agent() && caller.session().is_some()`; `with_path`: `matches!(caller, Caller::Owner | Caller::OwnerAgent { .. })`; `db_caller::caller_of` takes `&Caller` and for a `PersonAgent` returns `{level: rights.db_level(aid), viewer: Some(person)}`); `crates/clax-server/src/routes/mcp.rs` (`/mcp` refuses grants); `crates/clax-server/src/routes/mod.rs` (`mod agents`)
- Create: `crates/clax-server/tests/common/people.rs` (`World`), `crates/clax-server/tests/common/clients.rs` (`RealClients`), `crates/clax-server/tests/api_canary.rs`

**Interfaces (produces):** `need!` declarations, one per route row of spec §7.5 (e.g. `need!(ThreadsRead, Comments, "read", Filtered)`), exported as `agent::ROUTE_NEEDS: &[(&str /* handler */, Option<(RightType, &str)>, Class)]` for the contract generator (Task 24) and the sweep; `World::two_people()` seeding Mia's canaries in every author field of spec §8.2, including `capabilities`, a document path and a file the owner's agent carried forward from Mia's version (`World::canaries() -> Vec<&'static str>`); `RealClients` runs the built `clax` binary as the MCP shim (stdio), as `clax hook --agent claude stop|prompt|session-start` with harness JSON on stdin, and as `clax feedback follow --once`, plus `/mcp` over HTTP and Pi's extension under `node --test`, each against the test daemon, returning everything they print.

- [ ] **Step 1: Write the failing tests**

Guard (`routes/mod.rs`, reusing `l10::routes`, `l10::handlers`, `l10::function` made `pub(super)`):

```rust
#[test]
fn every_route_is_classified_for_agents() {
    // For every route of mod.rs and the /mcp service:
    // 1. its handler's signature has exactly one of `Agent<`, `DeniedToGrants`, `GrantFlow`;
    // 2. for `Agent<N>`, N appears in agent::ROUTE_NEEDS with the class and right of spec §7.5's row for this path
    //    (the expected table is copied into this test as data: path, method, class, right);
    // 3. Filtered: the handler source passes its response through `vis.filter_`;
    // 4. Own: the handler source builds none of thread_view(, comment_view(, artifact_view(, version_view(,
    //    doc_view(, working_view(;
    // 5. ROUTE_NEEDS has no entry no route uses.
}

#[test]
fn no_handler_reads_credentials_itself() {
    for (m, src) in all_route_module_sources() {
        assert!(!src.contains("has_token("), "{m} calls has_token: use Caller");
        assert!(!src.contains("AUTHORIZATION"), "{m} reads Authorization: use Caller");
    }
}
```

Canary harness (`tests/api_canary.rs`), generated from `agent::ROUTE_NEEDS`:

```rust
#[tokio::test]
async fn no_canary_reaches_an_unapproved_agent() {
    let w = World::two_people().await; // owner; Sam and Mia connected; Sam approved the owner (connect default); Mia's
                                       // canaries in every author field; Sam's agent; an owner agent
    for call in w.every_agent_call() { // each ROUTE_NEEDS route with real IDs: reads, writes (on scratch copies), error paths
        for agent in [&w.sam_agent, &w.owner_agent] {
            let body = call.run(agent).await;
            for c in w.canaries() {
                assert!(!body.contains(c), "{} leaked {c} to {}", call.name, agent.label);
            }
        }
    }
}

#[tokio::test]
async fn no_canary_reaches_an_agent_through_a_real_client() {
    let w = World::two_people().await;
    for out in RealClients::new(&w).run_every_channel().await { // shim tools, /mcp tools, hooks, feedback follow, Pi
        for c in w.canaries() { assert!(!out.text.contains(c), "{} leaked {c}", out.channel); }
    }
}
```

In this task `Filtered` routes answer `PersonAgent` 503 `not_yet` (so the sweep passes for Sam's agent trivially) and owner agents still get today's unfiltered content: the owner-agent half of the HTTP sweep and the whole real-client sweep are `#[ignore]`d with the reason "lifted by Task 12" (Pi's part by Task 19) and un-ignored there.

- [ ] **Steps 2–4:** fail, implement, pass (`cargo test -p clax-server`; every existing suite unchanged).
- [ ] **Step 5: Commit** — `"Declare every route's agent right and filter class on its handler, guard them, and sweep every route for leaked content"`

---

### Task 7: Agent names and `@name`

Spec §11, M22, R3, §9.6 (name rules).

**Files:**
- Create: `crates/clax-core/src/names.rs`
- Modify: `crates/clax-core/src/store/sessions.rs` (name on registration; `set_session_name`; names freed on end), `crates/clax-core/src/store/viewers.rs` (display names: refuse a LAN viewer's name equal to a person's or the owner's; refuse a person's name whose token form is a live agent's), `crates/clax-core/src/mentions.rs` (`agent_mentions(body, live_names) -> Vec<String>`), `crates/clax-server/src/routes/threads.rs` (`@name` sends with `to` and gives a watch; `unresolved_mentions`; a page's `@name` sends nothing), `crates/clax-server/src/routes/sessions.rs` (`PATCH` `name`), `crates/clax-server/src/routes/people.rs` (create it, with `PUT /api/agents/{handle}` rename; Tasks 16 and 17 extend it)
- Test: `names.rs`, `store/sessions.rs`, `tests/api_threads.rs`, `tests/api_sessions.rs`, `tests/api_viewers.rs`

**Interfaces (produces):** `names::derive(repo, branch, harness) -> String`, `names::sanitize(&str) -> Option<String>`, `names::token_form(display_name) -> String`, `names::is_reserved(name, people: &[String]) -> bool`; `Store::set_session_name(sid, name, NameSource) -> Result<Session>` (`name_taken`, `name_locked`, `invalid_name`); `mentions::agent_mentions`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn names_derive_from_repo_and_branch() {
    assert_eq!(derive(Some("clax"), Some("main"), "claude"), "clax");
    assert_eq!(derive(Some("clax"), Some("feature/Multi Person"), "codex"), "clax-multi-person");
    assert_eq!(derive(None, None, "pi"), "pi");
    assert_eq!(derive(Some(&"x".repeat(80)), None, "claude").len(), 32);
    assert_eq!(sanitize("Fix: chart axis!"), Some("fix-chart-axis".into()));
    assert!(sanitize("---").is_none());
    assert_eq!(token_form("Mia Kovač"), "mia-kovac");
    assert!(is_reserved("agent", &[]) && is_reserved("sam", &["Sam".into()]) && !is_reserved("clax-main", &["Sam".into()]));
}

#[test]
fn collisions_suffix_derived_names_and_refuse_chosen_ones() {
    let st = store();
    let a = st.register_session(reg_repo("claude", "clax", "main")).unwrap();
    let b = st.register_session(reg_repo("codex", "clax", "main")).unwrap();
    let c = st.register_session(reg_repo("codex", "clax", "main")).unwrap();
    assert_eq!((a.name.as_deref(), b.name.as_deref(), c.name.as_deref()), (Some("clax"), Some("clax-codex"), Some("clax-2")));
    assert_invalid(st.set_session_name(&b.id, "CLAX", NameSource::Agent), "name_taken");
    st.set_session_name(&b.id, "chart-axis", NameSource::Person).unwrap();
    assert_invalid(st.set_session_name(&b.id, "other", NameSource::Agent), "name_locked");
    st.end_session(&a.id).unwrap();
    st.set_session_name(&c.id, "clax", NameSource::Agent).unwrap();
}

#[test]
fn people_and_agents_cannot_shadow_each_other() {
    /* a live agent "sam" → naming a new person "Sam" is name_taken; a person "Mia" → an agent cannot be "mia";
       a LAN viewer cannot set display name "mia" (any case or diacritics) */
}
```

`tests/api_threads.rs`: `at_name_sends_to_exactly_that_agent_and_gives_it_a_watch`, `unknown_at_name_sends_nothing_and_says_so`, `page_written_at_name_sends_nothing`, `at_agent_keeps_its_meaning`. `tests/api_sessions.rs`: `registration_returns_a_name`, `patch_sets_the_agents_own_name`.

- [ ] **Steps 2–4**, **Step 5: Commit** — `"Name agents from their repository and branch, let agents and people rename them, and route @name to one agent"`

---

### Task 8: Visibility core, migration 23, document author sets and the generation counter

Spec §5.2, §8.1–§8.3, §8.7, M9–M14, M17 (with the bumps for folds and person revocation).

**Files:**
- Modify: `crates/clax-core/src/visibility.rs` (the filter), `crates/clax-core/src/store/migrations.rs` (migration 23), `crates/clax-core/src/store/docs.rs` (`doc_authors` on every write: every write adds the writer, the set only grows until delete; page writes add both the browser viewer and the page's publisher), `crates/clax-core/src/store/artifacts.rs` (`versions.file_authors_json` at publish: written files get the publisher (and `page_person`), carried files keep the previous version's authors), `crates/clax-core/src/store/threads.rs` (`comments.page_person` on page writes), `crates/clax-core/src/db.rs` (`Caller` for docs gains `person` and `page_person`), `crates/clax-core/src/store/viewers.rs` (fold covers `doc_authors.author`, `approvals.approver/author`, `approval_requests.approver/author`, `comments.page_person`), `crates/clax-server/src/state.rs` (`VisGen`)
- Create: `crates/clax-core/src/store/approvals.rs` (approvals only; requests in Task 14)

**Interfaces (produces):**
- `pub enum Author { Owner, Person(String), Unconnected, System }`
- `pub struct Visibility`: `all()`, `for_reader(&Store, &Reader, generation: u64) -> Result<Visibility>`, `sees(&self, authors: &[Author], artifact: &str, thread: Option<&str>) -> bool`, `withheld_authors(…) -> Vec<Author>`, `refresh_if_stale(&mut self, &Store, current: u64) -> Result<()>`, `is_all()`.
- Per-field author functions: `authors_of_comment`, `authors_of_anchor` (the anchored file's authors), `authors_of_file(version, path)`, `authors_of_clip`, `authors_of_resolved_by`, `authors_of_batch_note`, `authors_of_version` (files, label, note), `authors_of_artifact_text` (title, description, icon), `authors_of_asset`, `authors_of_doc`, `authors_of_working`.
- `placeholder` module: one function per spec §8.3 shape, used by Tasks 9–12.
- `Store::approve(approver, author, Scope, Via) -> Result<Approval>`, `revoke_approval(id, approver)`, `live_approvals(approver)`; `Store::people_present() -> Result<bool>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn own_person_is_always_visible_and_others_need_approval() {
    let st = two_people(); // owner, Sam and Mia (persons), Lan (unconnected viewer); migration 23 applied
    let sam = Visibility::for_reader(&st, &Reader::Person(SAM.into()), 0).unwrap();
    assert!(sam.sees(&[Author::Person(SAM.into())], "a1", Some("t1")));
    assert!(!sam.sees(&[Author::Person(MIA.into())], "a1", Some("t1")));
    assert!(!sam.sees(&[Author::Owner], "a1", None));
    assert!(!sam.sees(&[Author::Unconnected], "a1", None), "*unconnected is the owner's approval only");
    let owner = Visibility::for_reader(&st, &Reader::Owner, 0).unwrap();
    assert!(owner.sees(&[Author::Unconnected], "a1", None));
    assert!(!owner.sees(&[Author::Person(SAM.into())], "a1", None));
}

#[test]
fn scopes_thread_artifact_everywhere_and_revocation() { /* as revision 1's test, with R1: thread scope covers later replies */ }

#[test]
fn every_author_of_a_field_must_be_visible() {}

#[test]
fn page_written_content_counts_its_publisher_fixed_at_write() {
    let st = two_people();
    let (aid, tid, cid) = sam_page_with_owner_page_comment(&st); // v1 by Sam's agent; a via_page comment by the owner's browser
    st.publish_as_owner(&aid, "<p>v2</p>").unwrap();            // the owner publishes v2 afterwards
    let authors = authors_of_comment(&st.comment_row(&cid).unwrap());
    assert_eq!(authors, vec![Author::Owner, Author::Person(SAM.into())], "fixed at write, not the current publisher");
}

#[test]
fn touching_a_field_never_takes_a_document() {
    let st = two_people();
    st.doc_set_as(MIA, "a1", "notes/1", json!({"text": "MIA", "n": 1})).unwrap();
    st.doc_update_as(SAM, "a1", "notes/1", json!({"n": 2})).unwrap();
    assert_eq!(st.doc_authors("a1", "notes/1").unwrap(), vec![Author::Person(MIA.into()), Author::Person(SAM.into())]);
    st.doc_set_as(SAM, "a1", "notes/1", json!({"text": "mine"})).unwrap();
    assert_eq!(st.doc_authors("a1", "notes/1").unwrap(), vec![Author::Person(MIA.into()), Author::Person(SAM.into())],
               "a full set adds, never replaces");
}

#[test]
fn carrying_a_file_forward_keeps_its_author() {
    let st = two_people();
    let aid = st.publish_as(MIA, &[("index.html", "<p>MIA</p>"), ("app.css", "a{}")]).unwrap();
    st.publish_version_as_owner(&aid, &[("app.css", "b{}")]).unwrap(); // carries index.html forward
    assert_eq!(authors_of_file(&st.version(&aid, 2).unwrap(), "index.html"), vec![Author::Person(MIA.into())]);
    assert_eq!(authors_of_file(&st.version(&aid, 2).unwrap(), "app.css"), vec![Author::Owner]);
}

#[test]
fn folds_and_person_revocation_bump_the_generation() {}

#[test]
fn a_stale_visibility_refreshes_on_a_new_generation() {
    let st = two_people();
    let mut v = Visibility::for_reader(&st, &Reader::Person(SAM.into()), 1).unwrap();
    let id = st.approve(Some(SAM), MIA, Scope::Everywhere, Via::Shell).unwrap().id;
    v.refresh_if_stale(&st, 2).unwrap();
    assert!(v.sees(&[Author::Person(MIA.into())], "a1", None));
    st.revoke_approval(&id, Some(SAM)).unwrap();
    v.refresh_if_stale(&st, 3).unwrap();
    assert!(!v.sees(&[Author::Person(MIA.into())], "a1", None));
}

#[test]
fn single_owner_install_takes_the_fast_path() {}

#[test]
fn for_reader_reads_approvals_with_one_indexed_query() {}
```

- [ ] **Steps 2–4**, **Step 5: Commit** — `"Add the per-field visibility filter, approvals, document author sets and a generation counter (migration 23)"`

---

### Task 9: Filter threads, clips and the thread write routes

Spec §7.5 (thread rows), §8.3 (thread, comment), M10; review C2, C3.

**Files:** Modify `crates/clax-server/src/feedback.rs` (thread and comment views take `&Visibility` and filter every field), `crates/clax-server/src/routes/threads.rs` (`list`, `get`, `clip`, `create`, `comment`, `resolve`, `reopen`, `send`, `send_batch`), `crates/clax-server/src/agent.rs` (`FILTERED_LIVE` adds these handlers). Test: `crates/clax-server/tests/api_visibility.rs` (thread part), the canary sweep.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn visible_comment_on_withheld_page_hides_quote_and_title() {
    let w = World::two_people().await;                    // Sam approved the owner at connect, not Mia
    let (aid, tid) = w.owner_comments_on(&w.mia_page, "please tighten").await;
    let t = w.sam_agent.thread(&aid, &tid).await;
    assert_eq!(t["comments"][0]["body"], "please tighten");
    assert!(t["anchor"]["quote"].is_null() && t["anchor_withheld"] == true);
    assert_eq!(t["clip_withheld"], true);
    assert_eq!(t["withheld"]["authors"][0]["name"], "Mia");
}

#[tokio::test]
async fn write_routes_return_filtered_threads() {
    /* Sam's agent replies on a thread holding Mia's comment: the 201 body's thread has Mia's body null;
       resolve and reopen likewise; a send's response likewise */
}

#[tokio::test]
async fn owner_agent_sees_placeholder_for_page_written_comment() {}

#[tokio::test]
async fn people_are_never_filtered() { /* Sam's browser, the owner's browser and a LAN viewer see Mia's text */ }
```

The canary sweep (`no_canary_reaches_an_unapproved_agent`) now runs these routes for Sam's agent without `not_yet`.

- [ ] **Steps 2–4**, **Step 5: Commit** — `"Filter thread views field by field on every thread route an agent reaches, reads and writes"`

---

### Task 10: Filter artifacts, versions, files, assets and working records

Spec §7.5, §8.2–§8.3 (artifact, version, asset, working), O9.

**Files:** Modify `crates/clax-server/src/routes/{artifacts,content,assets,working}.rs` (list leaves out what the agent may not `read`; title, description, icon, labels, notes, file names, asset names and working messages filtered; file and blob content 403 `withheld`; publish conflict summaries filtered), `crates/clax-server/src/agent.rs` (`FILTERED_LIVE`). Test: `api_visibility.rs` (artifact part), the sweep.

- [ ] **Step 1: Write the failing tests:** `titles_are_withheld_in_list_and_read` (O9), `list_leaves_out_unreadable_artifacts` (Sam narrowed to one artifact), `version_files_and_notes_are_withheld`, `embedded_working_messages_are_filtered`, `publish_conflict_summary_is_filtered`, `asset_names_and_bytes_are_withheld`.
- [ ] **Steps 2–4**, **Step 5: Commit** — `"Filter artifacts, versions, files, assets and working messages for agents, and list only readable artifacts"`

---

### Task 11: Filter documents

Spec §8.2 (document authors), §8.3 (document, `where`/`order_by`); review I7.

**Files:** Modify `crates/clax-core/src/store/docs.rs` (`list_docs_visible(aid, filters, order, vis) -> (Vec<Doc>, withheld_count)`: filters and order evaluated over visible documents only), `crates/clax-server/src/routes/docs.rs` (every route returns filtered documents), `crates/clax-server/src/agent.rs`. Test: `crates/clax-core/src/store/docs.rs` tests, `api_visibility.rs`, the sweep.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn where_runs_over_visible_documents_only() {
    let st = two_people();
    st.doc_set_as(MIA, "a1", "c/1", json!({"secret": "aaa"})).unwrap();
    st.doc_set_as(MIA, "a1", "c/2", json!({"secret": "zzz"})).unwrap();
    st.doc_set_as(SAM, "a1", "c/3", json!({"secret": "mmm"})).unwrap();
    let vis = Visibility::for_reader(&st, &Reader::Person(SAM.into()), 0).unwrap();
    for w in [json!({"secret": {"$lt": "b"}}), json!({"secret": {"$gt": "y"}}), json!({})] {
        let (docs, withheld) = st.list_docs_visible("a1", "c", &w, None, &vis).unwrap();
        assert!(docs.iter().all(|d| d.path == "c/3") && withheld == 2, "{w}: the result never depends on Mia's values");
    }
}
```

`api_visibility.rs`: `patch_returns_filtered_documents`, `batch_results_are_filtered`, `content_dependent_writes_on_withheld_documents_are_refused` (`str_replace`, conditional `acquire` and the matching batch operations: 403 `withheld` whether or not the text would match; blind `set`, `patch`, `delete` allowed), `withheld_documents_are_counted_not_listed` (no withheld path in a listing).
- [ ] **Steps 2–4**, **Step 5: Commit** — `"Filter documents for agents and evaluate queries over visible documents only"`

---

### Task 12: Filter every delivery path, field by field

Spec §8.4 (waiters register before taking), M16, M17, M8 (agent channels).

**Files:** Modify `crates/clax-core/src/store/feedback.rs` (`take_feedback(&self, t, vis: &mut Visibility) -> TakeResult`; `take_notices(…, vis)`; each `FeedbackItem` field filtered; placeholders and `placeholder_at`; resends of visible bodies only; held rows on unreadable artifacts), `crates/clax-core/src/feedback.rs` (placeholder lines, filtered block lines, `FeedbackState::Withheld`), `crates/clax-server/src/routes/feedback.rs` (every tier; `refresh_if_stale` on every wake with `VisGen`), `crates/clax-server/src/push.rs` (`codex queue` payload), `crates/clax-server/src/feedback.rs` (`feedback_state` shows `withheld`), `crates/clax-server/src/agent.rs` (`FILTERED_LIVE` adds `feedback::poll`, `feedback::notices`), `crates/clax-server/src/feedback.rs` waiters (register, then take, then wait), `tests/api_canary.rs` (un-ignore the owner-agent half and the real-client sweep, Pi's part excepted). Test: `store/feedback.rs` tests, `api_visibility.rs`, a guard test in `clax-core`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn feedback_items_are_built_only_under_visibility() {
    // Scans clax-core/src for "FeedbackItem {" and "Notice {" constructors; only take_feedback, take_notices and
    // feedback.rs's tests may build them.
}

#[test]
fn withheld_bodies_stay_undelivered_and_get_one_placeholder() {
    let st = two_people();
    let (sid, tid) = mia_sends_to_sams_agent(&st, 2);
    let mut vis = Visibility::for_reader(&st, &Reader::Person(SAM.into()), 0).unwrap();
    let r = st.take_feedback(take(&sid, Tier::Piggyback), &mut vis).unwrap();
    assert!(r.items.is_empty());
    assert_eq!((r.placeholders.len(), r.placeholders[0].count), (1, 2));
    assert!(st.take_feedback(take(&sid, Tier::Piggyback), &mut vis).unwrap().placeholders.is_empty(), "once");
    st.approve(Some(SAM), MIA, Scope::thread_of(&st, &tid), Via::Shell).unwrap();
    vis.refresh_if_stale(&st, 1).unwrap();
    assert_eq!(st.take_feedback(take(&sid, Tier::Piggyback), &mut vis).unwrap().items.len(), 2);
}

#[test]
fn a_visible_body_is_delivered_with_its_withheld_fields_placeholdered() {
    let st = two_people();
    let sid = owner_sends_to_sams_agent_on_mias_page(&st, "fix the axis");
    let mut vis = Visibility::for_reader(&st, &Reader::Person(SAM.into()), 0).unwrap();
    let r = st.take_feedback(take(&sid, Tier::Piggyback), &mut vis).unwrap();
    let text = feedback::render(&r);
    assert!(text.contains("fix the axis") && text.contains("Anchored on: (withheld: page by Mia)") && text.contains("Clip: withheld"));
    assert!(!text.contains(MIA_TITLE) && !text.contains(MIA_QUOTE));
}

#[test]
fn revocation_holds_undelivered_rows_again_and_tells_once() {}
```

`api_visibility.rs`: `hook_tiers_are_filtered_for_owner_agents` (the real `clax hook` stop and prompt commands and `clax feedback follow`, run as processes with the daemon token as they are installed, carry no canary of Mia's), `mcp_tools_are_filtered_for_the_owner` (`/mcp`'s `comments_read`, `read`, `list`), `a_bump_between_take_and_wait_is_not_missed` (paused time: approve between the take and the wait; the poll wakes), `withheld_matrix_tiers` driven through `RealClients` (piggyback through the shim, stop and prompt hooks through the hook binary, `wait_for_feedback`, Pi inject, notices through `feedback follow`, `codex queue` through `clax-fake-exe` recording `--message`): no canary before approval, the placeholder exactly once per in-band tier, the content after; `revocation_during_wait_for_feedback_holds_rows` (a waiting poll; revoke; then a new send by Mia: the wake delivers only the placeholder); `approval_wakes_a_waiting_poll`.

- [ ] **Steps 2–4**, **Step 5: Commit** — `"Filter every feedback tier and notice field by field, hold withheld bodies with one placeholder, and follow approval changes during long-polls"`

---

### Task 13: The per-person inbox and person-scoped questions (migration 24)

Spec §5.3, M21, §13 (grouping data), §18. (Before approvals, so Task 14 can make `approval` items.)

**Files:** Modify `crates/clax-core/src/store/migrations.rs` (migration 24), `crates/clax-core/src/store/inbox.rs` (recipient on every kind; keys `…@<public ID>` for other people; kinds `approval`, `connection`, `ended`; participation for connected people; `connection` items for pending grants and invite redemptions, which Task 4's routes now also make), `crates/clax-core/src/store/questions.rs` (by the asking session's person), `crates/clax-server/src/routes/{inbox,questions}.rs` (`Identity::person()` scoping; the owner's routes unchanged), `crates/clax-server/src/stream.rs` (topics for people's streams, routed by recipient), `crates/clax-server/src/routes/mod.rs` (`l10::Guard::Owner` entries for these become `Person`, asserting `who.person(` and a recipient filter). Test: `store/inbox.rs`, `tests/api_inbox.rs`, `tests/api_questions.rs`.

- [ ] **Step 1: Write the failing tests:** `migration_24_keeps_seq_ids_keys_read_state_and_search` (as revision 1), `items_go_to_their_recipient` (a pending grant → `connection` for the owner; Sam's agent asks → Sam; Mia replies in a thread Sam is in → Sam and, when in it, the owner; Sam's agent publishes → Sam and the owner), `every_inbox_query_still_uses_an_index`, `a_person_sees_only_their_inbox`, `the_owner_inbox_is_unchanged`, `a_persons_agent_asks_its_person` (the owner gets 404 for it), `a_revoked_person_loses_the_inbox`.
- [ ] **Steps 2–4**, **Step 5: Commit** — `"Give each connected person an inbox, and route questions and connection requests to theirs (migration 24)"`

---

### Task 14: Approval requests, `ask` for approval, routing, deciding and revoking

Spec §9, §10, M18–M20, R1, R2.

**Files:** Modify `crates/clax-core/src/store/approvals.rs` (requests, batching, `decide`, withdrawals, the 100 limit), `crates/clax-core/src/store/feedback.rs` (`send_to` returns routing requests), `crates/clax-server/src/routes/approvals.rs` (new; every change requires `Caller::browser()` → 403 `browser_required`), `crates/clax-server/src/routes/threads.rs` (send, batch send, `@name`, a page's `sendToClaude` make routing requests; `agent_cannot_read`), `crates/clax-server/src/routes/sessions.rs` (session approval routes; waits reuse `QuestionWaiters`), `crates/clax-server/src/stream.rs` (`approval_request` on the approver's topics), `VisGen` bumps on every decision and revocation. Test: `tests/api_approvals.rs`.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn routing_to_another_persons_agent_makes_a_request() {
    let w = World::two_people().await;
    let mut sam_inbox = w.sam_browser.stream(&["inbox"]).await;
    let t = w.mia_browser.thread_on(&w.owner_page, "please fix the chart").await;
    w.mia_browser.send(&w.owner_page, &t, Some(&w.sam_agent.handle)).await;
    let item = sam_inbox.next_named("inbox_item").await;
    assert_eq!(item["item"]["kind"], "approval");
    assert!(w.sam_agent.feedback_text().await.contains("Your person has been asked."));
    assert_eq!(w.owner_browser.thread(&w.owner_page, &t).await["feedback_state"]["phase"], "withheld");
}

#[tokio::test]
async fn approval_changes_need_a_browser() {
    let w = World::two_people().await;
    let rid = w.open_request_for_owner().await; // Mia → the owner's agent
    for (m, p, b) in [("POST", format!("/api/approvals/requests/{rid}/decide"), json!({"scope": "everywhere"})),
                      ("POST", "/api/approvals".into(), json!({"author": MIA, "scope": "everywhere"})),
                      ("DELETE", format!("/api/approvals/{}", w.unconnected_approval_id().await), json!(null))] {
        let r = w.ts.token_call(m, &p, b).await; // the daemon token alone, as an owner agent could
        assert_eq!((r.status().as_u16(), code(r).await.as_str()), (403, "browser_required"), "{m} {p}");
    }
}

#[tokio::test]
async fn a_page_cannot_decide_an_approval() {
    /* the owner cookie (and a person's cookie) with Origin of an artifact subdomain, Origin null (sandboxed frame), or
       Sec-Fetch-Site same-site → 403 forbidden_origin on decide, allow and revoke */
}
#[tokio::test]
async fn ask_for_approval_waits_for_the_decision() {}
#[tokio::test]
async fn requests_batch_per_author_per_thread() {}
#[tokio::test]
async fn only_the_approver_decides() {}
#[tokio::test]
async fn decline_keeps_rows_held_and_asks_again_only_after_ten_minutes() {}
#[tokio::test]
async fn approving_everywhere_closes_the_authors_other_requests() {}
#[tokio::test]
async fn a_send_to_an_agent_that_cannot_read_is_refused() {}
#[tokio::test]
async fn a_page_send_routes_with_the_publisher_as_author() {}
#[tokio::test]
async fn the_hundred_and_first_request_is_refused_with_a_banner() {}
```

- [ ] **Steps 2–4**, **Step 5: Commit** — `"Turn withheld content into batched approval requests, from routing and from ask, decided in the agent's person's browser"`

---

### Task 15: Thread ownership, handoff and `thread_taken`

Spec §12, M23, M24, O8.

**Files:** Modify `crates/clax-core/src/store/feedback.rs` (first take sets the owner, deletes other sessions' undelivered rows), `crates/clax-core/src/store/threads.rs` (`thread_owner`, `handoff`, `check_not_taken`), `crates/clax-server/src/routes/threads.rs` (`thread_taken`; `owner`, `taken_by` in filtered views), `crates/clax-server/src/routes/sessions.rs` (`POST /api/sessions/{id}/threads/{tid}/handoff`, `Agent<ThreadsReply>`, Filtered). Test: `store/feedback.rs`, `tests/api_threads.rs`.

- [ ] **Step 1: Write the failing tests:** `first_take_has_one_winner` (two threads taking at once; exactly one gets the comment), `a_placeholder_take_makes_no_owner`, `non_owner_reply_is_thread_taken`, `owner_ended_threads_accept_any_agent`, `handoff_to_another_persons_agent_makes_a_routing_request`, `handoff_resends_and_release_untargets`, `views_show_owner_and_taken_by`.
- [ ] **Steps 2–4**, **Step 5: Commit** — `"Give each sent thread one owner agent, refuse other agents' writes, and add handoff and release"`

---

### Task 16: Lifecycle cleanup, remote idle and the agents roster

Spec §14, §13, M25.

**Files:** Modify `crates/clax-core/src/store/sessions.rs` (`end_session_touched` returns released threads, withdrawn requests, cleared rights, and writes the `ended` item; the reaper never PID-probes `grant_id` sessions and ends them after 30 minutes idle), `crates/clax-server/src/routes/people.rs` (`GET /api/agents`), `crates/clax-server/src/stream.rs` (`Topic::Agents`), `crates/clax-server/src/working.rs` (states), Send menus list live agents only. Test: `store/sessions.rs`, `tests/api_agents.rs`.

- [ ] **Step 1: Write the failing tests:** `ending_a_session_cleans_up_and_leaves_one_note`, `an_idle_session_ends_without_a_note`, `remote_sessions_are_never_pid_probed_and_idle_thirty_minutes` (ManualClock: 29 min live, 31 min ended), `revoking_a_grant_ends_its_sessions_with_their_notes`, `roster_reports_blocked_since_and_is_person_scoped`, `reregistering_after_an_idle_end_restores_watches_and_says_what_ended` (same `harness_session_id` and grant within 24 h: watches back, the first tool result's note names the released threads; another grant's registration with that ID restores nothing).
- [ ] **Steps 2–4**, **Step 5: Commit** — `"Clean up after ended agents with one inbox note, keep remote sessions through short sleeps, and serve an agent roster"`

---

### Task 17: People, rights, narrowing, elevation and the `*unconnected` switch

Spec §7.4, §9.6, §6.4 (revoking a person), M20.

**Files:** Modify `crates/clax-server/src/routes/people.rs` (`GET /api/people`, `GET /api/people/me`, `PUT /api/people/{pid}/rights`, `DELETE /api/people/{pid}`, `PUT /api/people/me/grants/{gid}/narrow`, `PUT|DELETE /api/agents/{handle}/elevate`, `PUT /api/agents/{handle}/narrow`; every change `Caller::browser()` and `SameOrigin`; each bumps `VisGen`), `crates/clax-server/src/routes/approvals.rs` (the switch is the `*unconnected` approval), `crates/clax-server/src/extension.rs` (the gateway admits `GET /api/approvals/requests` for the owner: the panel's count and links), author lines `(not connected)` in `feedback.rs` and views. Test: `tests/api_people.rs`, the extension gateway rule table.

- [ ] **Step 1: Write the failing tests:** `rights_changes_need_a_browser` (token alone: 403 `browser_required` on rights, elevate, narrow, revoke person), `elevation_exceeds_the_person_for_one_agent_only_until_its_session_ends`, `narrowing_applies_at_the_next_request_and_cannot_widen`, `an_agent_cannot_change_its_own_rights` (403 `forbidden`), `revoking_a_person_revokes_grants_and_invites_and_keeps_history`, `people_routes_are_owner_or_self`, `unconnected_switch_off_withholds_lan_viewers_from_owner_agents`, `unconnected_authors_are_marked_not_connected`.
- [ ] **Steps 2–4**, **Step 5: Commit** — `"Let the owner set people's rights and elevate one agent, let people narrow their agents, all from the browser, and add the LAN switch"`

---

### Task 18: Remote mode and `clax connect`

Spec §16 (connect, remote mode), M26, §22.

**Files:** Create `crates/clax-cli/src/remote.rs` (`Remote`, `load(home)`, `SignedClient`: signs, retries once on `stale_request` with the `Date` offset), `crates/clax-cli/src/commands/connect.rs` (`<invite URL>`, `<daemon URL>`, `--status`, `--off`, `--url`, `--name`, `--no-browser`, `--yes`), `crates/clax-cli/src/commands/comments.rs` (remote mode lists through `GET /api/artifacts` and each artifact's threads, takes the name from `remote.json`), every other command (refuses with "not available on a remote Clax", exit 2, unless spec §6.7 lists it); modify `crates/clax-cli/src/client.rs` and `crates/clax-mcp/src/client.rs` (remote when `remote.json` exists and `CLAX_REMOTE` is not `0`; never auto-start then), `crates/clax-cli/src/commands/doctor.rs` (remote checks: reachable, key readable, clock offset, IP literal), `crates/clax-core/src/home.rs` (`remote_json()`, `remote_key()`, `remote_cache()`). Test: `crates/clax-cli/tests/connect.rs`, `remote.rs` unit tests.

- [ ] **Step 1: Write the failing tests:** `connect_with_an_invite_writes_remote_json_and_key_0600` (and `--yes` recorded the owner approval), `remote_mode_signs_every_request_and_never_starts_a_daemon`, `adhoc_connect_waits_and_completes_on_approval`, `connect_off_revokes_and_removes_the_files`, `url_rewrites_the_address_and_keeps_the_grant`, `remote_cli_commands_work_as_listed` (each command spec §6.7 lists, run remotely against a daemon with Mia's canaries: works, filtered for the person; `clax comments` without an artifact lists every readable artifact's threads), `other_commands_refuse_remotely` (exit 2 with the message).
- [ ] **Steps 2–4**, **Step 5: Commit** — `"Add clax connect and a signed remote mode for the CLI, hooks and shim"`

---

### Task 19: MCP tools, hooks and Pi for multiple agents and people

Spec §16 (tools, clips, hooks), §11, §9.2, §12.

**Files:** Modify `crates/clax-mcp/src/tools.rs` (`status` `name` and fields; `ask` `approval`, `approval_id`; `comments_handoff`; new error codes), `crates/clax-mcp/src/render.rs` (`clax-clip:` rewrite), `crates/clax-hooks/src/*.rs` (clip rewrite in Stop and prompt payloads; session lookup through the grant-scoped listing; `X-Clax-Channel: agent` on every request), `plugins/pi/src/{client,remote,clax}.ts` (remote mode, `node:crypto` Ed25519 from `remote.key`, `X-Clax-Channel: agent`, new arguments, `clax_comments_handoff`), `tests/api_canary.rs` (un-ignore Pi's part of the real-client sweep), `scripts/sync-skill-tools.py` (25 tools). Test: `tools.rs` tests, `tests/api_mcp.rs`, `plugins/pi/test/*.test.ts`.

- [ ] **Step 1: Write the failing tests:** `status_sets_and_reports_the_name`, `ask_with_approval_returns_the_decision`, `comments_handoff_moves_and_releases`, `remote_clip_lines_are_rewritten_to_cached_files` (a withheld clip rewrites to `Clip: withheld` with no fetch), `remote_hooks_find_their_session`, `tool_count_is_25`; Pi: `remote mode signs with the same canonical string as Rust` (Task 3's vector), `clax_comments_handoff is registered`.
- [ ] **Steps 2–4**, **Step 5: Commit** — `"Expose agent names, approval requests and thread handoff to agents in every harness, local and remote"`

---

### Task 20: The shell's People page

Spec §17 (People), §9.6, §3.1, §3.4. **Before starting, ask the owner the UI questions of spec §26 in one batch and record the answers in the task's report; begin the UI only once they are answered.**

**Files:** Create `web/shell/src/people/` (`People.svelte`, `PersonCard.svelte`, `InviteDialog.svelte`, `UnconnectedSwitch.svelte`, `model.ts`, tests beside each); modify `web/shell/src/api.ts`, the top bar (People link), `crates/clax-server/src/routes/shell.rs` (`/people` serves the gallery entry), `web/perf/bundle-budget.json` (`people`: 16384). Create `scripts/dev-second-person.sh` (connects a temporary home to the dev daemon and registers an agent).

- [ ] **Steps:** failing Vitest tests (`InviteDialog` shows the command, expiry, "Use a trusted LAN" and, on the first invite, the switch; `PersonCard` shows machines with fingerprints and agents; a person's own view hides other people; hostile names stay text); implement; **verify in a real browser** (`just dev claude` plus `scripts/dev-second-person.sh`; `/people` as owner and as the person; screenshots at 1280 and 390 px, light and dark); commit — `"Add the People page: invites, people, machines, agents, rights and the LAN switch"`

---

### Task 21: Approval and connection cards

Spec §9.5, §3.2, §3.3.

**Files:** Create `web/shell/src/people/ApprovalCard.svelte`, `ConnectionCard.svelte`; modify `web/shell/src/q/` (inbox renders `approval`, `connection`, `ended`; the card above a thread).

- [ ] **Steps:** failing tests (`ApprovalCard` shows author, agent, thread and content as text, the thread button says "including later replies", each button decides its scope; `ConnectionCard` enables **Approve** only with a typed code and a new name, links to **Invite** for existing people); implement; verify in a real browser with screenshots; commit — `"Add approval and connection cards to the inbox"`

---

### Task 22: Agent strip, owner chips and the Send menu

Spec §13, §17 (threads), §10.

**Files:** Create `web/shell/src/people/AgentStrip.svelte`; modify thread cards (owner chip, `withheld` delivery state), the Send menu (by name, grouped by person, "Sam approves first"), the artifact agent list.

- [ ] **Steps:** failing tests (`AgentStrip` orders blocked-longest first and formats "waiting on you 12 min" from a fake clock; the Send menu lists "Your agents" then "Sam's agents"); implement; verify in a real browser with screenshots; commit — `"Group the inbox by agent with block times, show thread owners, and name agents in the Send menu"`

---

### Task 23: The extension's panel

Spec §9.5 (extension: no approval cards), §17.

**Files:** Modify `web/extension/src/panel/` (an approval item in the Inbox tab links to the shell's inbox; agent names in the Send menu), `web/extension/src/sw/inbox.ts`.

- [ ] **Steps:** failing tests (`an approval item opens the shell's inbox`, `send menu names agents`); implement; verify in Chromium with the unpacked extension (screenshots); commit — `"Name agents in the extension's panel and link its approval items to the shell"`

---

### Task 24: Skills and contract

Spec §15, §16 (skills), §7.5.

**Files:** Modify `plugins/*/skills/clax/SKILL.md` ("Working beside other agents and people", including: do not open other people's Clax pages with web or browser tools; never fetch `/api/token`, run commands that change approvals or rights, or act in People), `docs/contract.md` (People and grants; rights; the route table generated from `agent::ROUTE_NEEDS` by a test that fails when the committed table differs; visibility and placeholders; approvals; names; ownership; remote mode; every new error code), `docs/superpowers/specs/2026-09-28-clax-design.md` (§1 "Out of scope": point to this spec).

- [ ] **Steps:** update; `scripts/sync-skill-tools.py --check`; the contract tests; commit — `"Document people, rights, the agent route table, visibility, names and ownership"`

---

### Task 25: Single-owner golden, perf baseline and budgets, two-person end to end

Spec §19, §23, §24.

**Files:** Modify `crates/clax-server/tests/golden_single_owner.rs` (Task 0's collector; add the comparison); create `web/e2e/visibility.spec.ts`, `web/e2e/people.spec.ts`; modify `scripts/perf-daemon.py` (a `vis_baseline` phase, a two-person seed, `vis_threads_ms`, `vis_feedback_poll_ms`, `sig_verify_us`, a signed polling client beside the probe loads), `scripts/perf-daemon-budget.json`.

- [ ] **Step 1: Write the failing tests**

```rust
/// Agent-facing outputs on a migrated single-owner install equal main's, apart from spec §19's allowlist.
#[tokio::test]
async fn single_owner_outputs_are_unchanged() {
    let golden: Value = serde_json::from_str(include_str!("fixtures/single-owner-21.golden.json")).unwrap();
    for tier in TIERS { // each tier on a fresh copy of the fixture: takes are destructive
        let ts = TestServer::from_fixture_with_clock("single-owner-21.sql", FIXED_T).await;
        let got = normalise_minted(collect_agent_outputs(&ts, tier).await);
        let want = expect_changes(golden[tier.name()].clone(), EXPECTED); // §19 items 1, 6, 7 applied to main's output
        assert_eq!(got, want, "{}", tier.name());
    }
}
```

`EXPECTED` applies exactly spec §19's item 1 (derived names where main showed the harness), item 6 (the additive fields with their solo values) and item 7 (`(not connected)` after each unconnected author) to the golden; nothing else is normalised besides Task 0's minted values; text payloads are compared exactly. Item 8 has its own test, `a_lan_viewer_cannot_take_the_owners_name`. `visibility.spec.ts`: the owner invites Sam in People; a signed test client connects as Sam and registers an agent; Mia (a second client) comments and sends to Sam's agent; Sam's browser (via the browser link) sees the approval card and approves the thread; the agent's feedback then carries the comment; screenshots at desktop and phone width. `people.spec.ts`: an ad-hoc request approved with the code into a new person.

- [ ] **Step 2:** fails. **Step 3:** record the perf baseline at Task 0's commit (`git worktree add` that commit in the scratch directory, add the `vis_baseline` phase there, run `scripts/perf-daemon.sh --phase vis_baseline`), and set the two budgets to baseline + 2 ms and + 1 ms. The golden itself was written at Task 0 and is never regenerated.
- [ ] **Step 4:** run `scripts/quality_gates.sh` (within about 2 minutes), `scripts/perf-daemon.sh`, `npm --prefix web run e2e -- visibility people`.
- [ ] **Step 5: Commit** — `"Pin single-owner behaviour with a golden, budget the filter from a measured baseline, and test two people end to end"`

---

## Audit adoption (no task; for the merge)

When `toolpath-audit` merges, each store function named in spec §20.2 already returns what its event needs (`RevokedGrant`, `RevokedPerson`, `Decided`, `TakeResult.placeholders`, `EndedSession`, …); the merge adds the `audit::record` call in the same transaction and the actor fields of §20.1. Whichever branch merges second renumbers its migrations and extends the audit backfill to grants, approvals and requests (`backfilled: true`).

## Self-review notes

- Decisions map to tasks: O1 → 2, 4, 18, 20, 21; O2 → 1, 5, 17; O3 → 0, 8–14, 19, 21, 25; O4 → 7, 15, 16, 22; O5 → 1–4 (spec §4); O6 → 8, 17; O7 → 20 (copy); O8 → 15; O9 → 10; R1 → 14, 21; R2 → 14; R3 → 7; R4 → none (out of scope).
- No commit serves unfiltered content to a person's agent: Task 5 answers every `Filtered` route 503 `not_yet` for `PersonAgent`, and Tasks 9–12 lift it route by route as each filters. Owner agents (the token) receive unfiltered other-people content between Task 4 and Task 12 only in tests: a person row can exist only through Task 4's routes, which nothing outside tests calls before the People page (Task 20). Between Tasks 4 and 9 a connected person's browser comments, recognised by `Caller` from Task 5, are attributed to that person and would reach the owner's agents unfiltered on such a branch build; this is a branch-only window.
- Task sizes: the former gateway task is split into Caller/auth (5) and classification/guards (6); the read filter into threads (9), artifacts (10) and documents (11); the shell into People (20), cards (21) and strip/chips/menu (22).
- The inbox's recipient (Task 13) lands before approval requests (Task 14), which make `approval` items.
