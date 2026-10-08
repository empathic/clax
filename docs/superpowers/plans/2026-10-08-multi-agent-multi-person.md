# Multi-agent and Multi-person Clax Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Several agents of one person, and the agents of a second person on another machine, share one Clax: the owner connects people by invite or approved request; each agent holds a key-bound credential tied to its person, with its person's rights (narrowable by the person, extendable per agent by the owner); content written by anyone else reaches an agent only after its person approves it, enforced by one daemon-side filter on every agent-facing read and delivery path; agents have names, `@name` reaches exactly one, a sent thread has one owner agent with explicit handoff; ended sessions clean up; the inbox and roster are grouped by agent and say who is blocked on the person.

**Architecture:** A person is a `viewers` row (`person = 1`); an agent is a `sessions` row bound to a person and, from another machine, to a GNAP-shaped grant (migration 22). Grants are made by invite or by an ad-hoc request the owner approves with the requester's user code; access tokens are opaque, hashed, rotated through a manage URI, and every request is signed with the machine's Ed25519 key (`clax-ed25519`). Rights are GNAP access objects in a pure `clax_core::rights` module. A new agent gateway middleware verifies grants, checks rights against an allowlist, and hands requests to the existing handlers, which take `RequireAgent` instead of `RequireToken`. Visibility (migration 23: approvals, approval requests, document authors) is one `clax_core::visibility::Visibility` built per request for the reading agent's person and used by every agent-facing read route and by `take_feedback` for every tier; withheld content becomes placeholders, routed sends become approval requests, approvals release held rows. Migration 24 rebuilds the inbox with a recipient. The agent side gains a remote mode (`remote.json`, signed requests) used by `clax connect`, the shim, hooks, the CLI and Pi. The shell gains a lazy People page, approval and connection cards, an agent strip and owner chips; the extension's panel gains approval cards.

**Tech Stack:** Rust 2024 (axum, tokio, rusqlite, serde, rmcp, clap, ring for Ed25519), Svelte 5 (runes) + TypeScript, Vite 6, Vitest + jsdom + @testing-library/svelte, Playwright (Chromium), Chrome MV3 extension, Pi extension (TypeScript, `node:crypto`).

**Spec:** `docs/superpowers/specs/2026-10-08-multi-agent-multi-person-design.md` (with the main spec, the chrome-overlay spec and the questions-and-inbox spec). Read it in full before Task 1; each task names the sections it implements.

## Global Constraints

- Owner decisions O1–O5 and design decisions M1–M24 are binding; a task that finds one unworkable stops and reports, it does not improvise.
- **Migration numbers.** This plan adds three migrations, written here as 22, 23 and 24 after main's 21. The `toolpath-audit` branch also claims 22: before Task 2 starts, check `MIGRATIONS.len()` on main; if the audit migration has landed, renumber these to follow it (and every comment, test name and spec reference with them). Never edit a migration that is on main.
- No `unsafe` Rust: every crate keeps `#![forbid(unsafe_code)]`. `ring` becomes a direct dependency of `clax-core` (it is already in `Cargo.lock`); no other new crypto crate.
- No hard links anywhere (`scripts/check-no-hard-links.sh` stays green); the remote clip cache writes files by copy only.
- **Zero behaviour change for single-owner installs** (spec §19): until a person row exists, every agent-facing output is byte-identical to main's except agent names in the shell and M18's first-take among several owner agents. Task 20's golden test pins it; any other diff is a defect.
- **The filter is agent-facing only.** People (the owner's browsers, connected people's browsers, LAN viewers) see exactly what they see today. Owner-token agent requests (token, no browser) are filtered for the owner.
- **One filter.** Only `clax_core::visibility::Visibility` decides visibility. No route or store function re-implements the rule. `FeedbackItem` and `Notice` are constructed only in functions that take `&Visibility` (Task 9's guard).
- **Agent credentials never act as the owner**, not even a grant whose person is the owner row; owner routes keep `require_owner`.
- Tokens, codes, keys, signatures and withheld content never appear in logs, errors, events or `Debug` output (`Secret` newtypes with redacting `Debug`).
- Untrusted text (person names, agent names, client names, comment bodies on approval cards): Svelte text interpolation only; CLI escapes control and bidirectional characters; payloads quote with `feedback::quoted`.
- Doc comments and commit messages describe the contract or the change, never a conversation; in prose write "ID", never "id", except as a literal symbol.
- Tests: the whole `scripts/quality_gates.sh` stays within about 2 minutes on a warm cache; fake clocks (`ManualClock`), injected durations or paused Tokio time; wait on events, never a fixed sleep. A new slow test is a defect.
- Time to usable: `web/perf/bundle-budget.json` keeps `gallery` and `artifact` within +512 bytes, adds `people` at 16384, keeps `extPanel` at 65536; `scripts/perf-daemon-budget.json` adds `vis_threads_ms: 15`, `vis_feedback_poll_ms: 5`, `sig_verify_us: 150`; `web/perf/usable.perf.ts` stays green.
- UI changes are verified in a real browser (load the route, screenshots at desktop and phone width, both themes) before a UI task is reported done; "tests pass" is not verification.
- Commit with `git -c commit.gpgsign=false commit …`; never `--no-gpg-sign`; never push or merge; never touch `~/.clax` (tests use temporary homes).

## Review Focus

1. **A withheld comment leaks through some path.** A person's agent that has not approved Mia must never receive Mia's words, her page's quote, her clip, her document, her version files or her artifact's title, through any route or tier (piggyback, stop hook, prompt hook, wait, inject, queue, notices). Pinned by Task 5's route guard (`every_route_is_classified_for_agents`), Task 9's delivery guard (`feedback_items_are_built_only_under_visibility`) and the matrix tests in Tasks 8 and 9 (`withheld_matrix_reads`, `withheld_matrix_tiers`).
2. **A page speaks for the owner.** A page published by Sam's agent writes a comment and a document through the owner's browser; the owner's agents must see placeholders until the owner approves Sam. Pinned in Task 7 (`page_written_content_counts_its_publisher`) and Task 8 (`owner_agent_sees_placeholder_for_page_written_comment`).
3. **A sniffed request or token is reused.** Replaying a signed request (same nonce), re-signing with another key, a stale timestamp, a token alone, a grant naming another grant's session: all refused. Pinned in Task 3 (`replay_and_staleness_are_refused`) and Task 5 (`foreign_session_is_refused`, `token_without_signature_is_refused`).
4. **The owner approves the wrong machine.** An ad-hoc request approved with a code that is not the requester's changes nothing; three wrong codes deny it; an invite redeemed twice fails the second time. Pinned in Task 4 (`wrong_code_approves_nothing`, `invite_is_single_use`).
5. **A single-owner install changes behaviour.** Pinned by Task 20 (`single_owner_outputs_are_unchanged`), which compares main's outputs with this branch's on a migrated fixture.
6. **Two agents answer one thread.** Racing first takes and a reply by a non-owner. Pinned in Task 12 (`first_take_has_one_winner`, `non_owner_reply_is_thread_taken`).

---

## File structure

```
crates/clax-core/src/rights.rs                  access objects, presets, intersect, union, allows
crates/clax-core/src/sig.rs                     clax-ed25519: canonical string, sign, verify, NonceCache
crates/clax-core/src/names.rs                   agent name derivation, sanitising, reserved words, @name parsing
crates/clax-core/src/visibility.rs              Visibility, Author, effective authors, placeholders
crates/clax-core/src/store/people.rs            person rows, invites, grants, tokens
crates/clax-core/src/store/approvals.rs         approvals, approval requests, release of held rows
crates/clax-core/src/store/migrations.rs        migrations 22, 23, 24
crates/clax-core/src/store/sessions.rs          grant/person/name columns, remote liveness, end cleanup
crates/clax-core/src/store/feedback.rs          take_feedback under Visibility, placeholders, first take
crates/clax-core/src/store/threads.rs           owner, handoff, thread_taken
crates/clax-core/src/store/inbox.rs             recipient, kinds approval/connection/ended
crates/clax-core/src/store/viewers.rs           fold follows person/author/approver columns
crates/clax-core/src/store/docs.rs              docs.author, via_page
crates/clax-core/src/store/artifacts.rs         artifacts.person, versions.person, by_page
crates/clax-server/src/agent_gateway.rs         the agent gateway, RequireAgent, ViaGrant, Reader
crates/clax-server/src/identity.rs              Identity::person, ViaGrant
crates/clax-server/src/routes/grants.rs         grant flow, invites, approvals of grants
crates/clax-server/src/routes/people.rs         people, rights, elevate, narrow, agents roster, rename
crates/clax-server/src/routes/approvals.rs      approvals and requests
crates/clax-server/src/routes/connect.rs        GET /connect/b/<code>
crates/clax-server/src/routes/*.rs              RequireAgent and Visibility on agent routes
crates/clax-server/src/routes/mod.rs            new routes; `mod agents` guard test
crates/clax-server/src/stream.rs                topics people, agents; person-scoped questions/inbox
crates/clax-server/src/extension.rs             gateway admits approval routes
crates/clax-mcp/src/client.rs, tools.rs         remote mode, status name, ask approval, comments_handoff, clip cache
crates/clax-cli/src/remote.rs                   remote.json, key, signing client, rotation
crates/clax-cli/src/commands/connect.rs         clax connect
crates/clax-cli/src/commands/people.rs          clax people, clax agents, clax approvals
crates/clax-cli/src/client.rs                   remote mode for every command and hook
plugins/pi/src/client.ts, remote.ts, clax.ts    remote mode, signing, new tool arguments
plugins/*/skills/clax/SKILL.md                  "Working beside other agents and people"
web/shell/src/people/                           People page, approval and connection cards, agent strip
web/shell/src/q/                                inbox: new kinds, agent strip mount
web/shell/src/ui/...                            owner chips, Send menu by name, People link
web/extension/src/panel/                        approval cards
web/e2e/people.spec.ts, visibility.spec.ts      end to end
scripts/perf-daemon.py, perf-daemon-budget.json two-person seed and budgets
docs/contract.md                                People, grants, rights, visibility, names, ownership
```

---

### Task 1: Rights

Spec §7, M5.

**Files:**
- Create: `crates/clax-core/src/rights.rs`
- Modify: `crates/clax-core/src/lib.rs` (`pub mod rights;`)

**Interfaces:**
- Produces:
  - `pub enum RightType { Artifacts, Comments, Db, Ask }` (serde `clax-artifacts` …)
  - `pub enum Location { Any, Own, Artifact(String) }` (serde `"*"`, `"own"`, an ID)
  - `pub struct Access { kind: RightType, actions: BTreeSet<String>, locations: Vec<Location>, level: Option<Level> }`
  - `pub struct Rights(Vec<Access>)` with `parse(&Value) -> Result<Rights>` (accepts an array of objects and preset strings; 400 `invalid_rights`), `preset(&str) -> Option<Rights>`, `full()`, `intersect(&self, &Rights) -> Rights`, `union(&self, &Rights) -> Rights`, `is_narrower_or_equal(&self, &Rights) -> bool`, `allows(&self, t: RightType, action: &str, artifact: Option<&str>, own: &dyn Fn(&str) -> bool) -> bool`, `db_level(&self, artifact: &str, own: &dyn Fn(&str) -> bool) -> Option<Level>`, `to_json()`.
  - `pub fn effective(person: &Rights, grant_narrow: Option<&Rights>, session_narrow: Option<&Rights>, elevate: Option<&Rights>) -> Rights`
  - consts `PRESET_READER`, `PRESET_COLLABORATOR`, `PRESET_FULL` (`"clax:reader"` …)

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
        assert_eq!(c.db_level("other", &own), Some(Level::Interact));
        assert_eq!(c.db_level("mine", &own), Some(Level::Admin));
        let r = Rights::preset(PRESET_READER).unwrap();
        assert!(!r.allows(RightType::Comments, "resolve", Some("x"), &own));
        assert!(Rights::full().allows(RightType::Artifacts, "delete", Some("x"), &own));
    }

    #[test]
    fn effective_is_person_narrowed_then_elevated() {
        let person = Rights::preset(PRESET_COLLABORATOR).unwrap();
        let no_delete = Rights::parse(&json!([{"type": "clax-artifacts", "actions": ["read", "create", "update"], "locations": ["*"]},
            {"type": "clax-comments", "actions": ["read", "reply", "resolve", "watch"], "locations": ["*"]},
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
        assert!(c.to_json().as_array().unwrap().iter().all(|a| a["type"].as_str().unwrap().starts_with("clax-")));
    }
}
```

- [ ] **Step 2: Run them to see them fail** — `cargo test -p clax-core rights::` (unresolved module).
- [ ] **Step 3: Implement** per spec §7.1–§7.3. `intersect` keeps, per type, actions present on both sides at locations both allow (`Any ∩ L = L`, `Own ∩ Artifact(a)` keeps `Artifact(a)` only when checked with `own(a)` at `allows` time, so represent it as a conjunction list rather than resolving early); `union` concatenates and dedups. `allows` walks the list.
- [ ] **Step 4: Run** — `cargo test -p clax-core rights::` passes; `cargo clippy -p clax-core -- -D warnings`.
- [ ] **Step 5: Commit** — `git -c commit.gpgsign=false commit -m "Add GNAP-shaped access rights with presets, narrowing and elevation"`

---

### Task 2: Migration 22 and the people, invite, grant and token store

Spec §5.1, §6.2–§6.4, M1–M3.

**Files:**
- Modify: `crates/clax-core/src/store/migrations.rs` (migration 22)
- Create: `crates/clax-core/src/store/people.rs`
- Modify: `crates/clax-core/src/store/mod.rs`, `crates/clax-core/src/model.rs` (`Viewer.person`, `Session.grant_id/person/name/name_source`), `crates/clax-core/src/store/viewers.rs` (fold rewrites `sessions.person`, `artifacts.person`, `versions.person`), `crates/clax-core/src/store/artifacts.rs` (write `artifacts.person`, `versions.person`, `by_page`)

**Interfaces:**
- Produces (`impl Store`):
  - `create_invite(&self, who: InvitePerson, now) -> Result<(Invite, Secret)>` where `InvitePerson::{New(name), Viewer(public_id)}`; `list_invites`, `revoke_invite(id)`.
  - `request_grant(&self, g: NewGrant) -> Result<GrantStart>`; `NewGrant {key: [u8; 32], client_name, peer_ip, access: Rights, invite: Option<String>}`; `GrantStart::{Approved {grant, token: Secret, continue_token: Secret, browser_code: Secret}, Pending {grant, continue_token: Secret, user_code: Secret}}`; errors `invalid_invite`, `limit_reached`.
  - `approve_grant(gid, user_code, person: GrantPerson, rights: Option<Rights>)` (`wrong_code`, three strikes deny), `deny_grant(gid, reason)`, `continue_grant(gid, continue_token) -> Result<ContinueOutcome>`, `rotate_token(gid, token) -> Result<Secret>`, `revoke_token`, `revoke_grant(gid, by) -> Result<RevokedGrant {sessions_ended}>`, `revoke_person(public_id)`, `narrow_grant(gid, Rights)`.
  - `token_index(&self) -> Result<Vec<([u8; 32], String)>>` (live token hash → grant ID, for the in-memory map).
  - `person_rights(public_id) -> Result<Rights>`; `set_person_rights(public_id, Rights)`.
  - `expire_grants(now) -> Result<Vec<String>>` (pending past 10 min, unused 30 days, tokens past 7 days).
  - `Secret(String)` with `Debug` printing `Secret(…)`.
- Consumes: Task 1 `Rights`.

- [ ] **Step 1: Write the failing tests** (`store/people.rs` tests, with `ManualClock`-driven `now` arguments)

```rust
#[test]
fn migration_22_from_21_keeps_rows_and_adds_owner_nulls() {
    let st = Store::open_at_version(tmp(), 21); // existing helper used by migration tests
    seed_21(&st);                                // a session, an artifact with 2 versions, a viewer
    st.migrate_to_latest().unwrap();
    let s = st.list_sessions(false).unwrap();
    assert!(s.iter().all(|s| s.person.is_none() && s.grant_id.is_none() && s.name.is_none()));
    assert!(st.list_artifacts_raw().unwrap().iter().all(|a| a.person.is_none()));
    assert!(!st.get_viewer(SEEDED).unwrap().unwrap().person);
}

#[test]
fn invite_is_single_use_and_binds_the_first_key() {
    let st = store();
    let (inv, code) = st.create_invite(InvitePerson::New("Sam".into()), T0).unwrap();
    let start = st.request_grant(grant(KEY_A, Some(code.expose()))).unwrap();
    let GrantStart::Approved { grant, .. } = start else { panic!() };
    assert_eq!(grant.status, "approved");
    let again = st.request_grant(grant(KEY_B, Some(code.expose())));
    assert_invalid(again, "invalid_invite");
    assert!(st.list_invites().unwrap().iter().any(|i| i.id == inv.id && i.used_at.is_some()));
}

#[test]
fn wrong_code_approves_nothing_and_three_deny() {
    let st = store();
    let GrantStart::Pending { grant, user_code, .. } = st.request_grant(grant(KEY_A, None)).unwrap() else { panic!() };
    for _ in 0..2 { assert_invalid(st.approve_grant(&grant.id, "AAAA-0000", GrantPerson::New("Sam".into()), None), "wrong_code"); }
    assert_eq!(st.grant(&grant.id).unwrap().status, "pending");
    assert_invalid(st.approve_grant(&grant.id, "AAAA-0000", GrantPerson::New("Sam".into()), None), "wrong_code");
    assert_eq!(st.grant(&grant.id).unwrap().status, "denied");
    let _ = user_code;
}

#[test]
fn pending_limits_per_ip_and_overall() {
    let st = store();
    for i in 0..3 { st.request_grant(grant_from(key(i), "10.0.0.9")).unwrap(); }
    assert_invalid(st.request_grant(grant_from(key(9), "10.0.0.9")), "limit_reached");
}

#[test]
fn tokens_rotate_with_a_grace_and_expire() {
    let st = store();
    let (gid, t1) = approved(&st);
    let t2 = st.rotate_token(&gid, t1.expose(), T0).unwrap();
    assert!(st.token_live(t1.expose(), T0 + 59).unwrap() && !st.token_live(t1.expose(), T0 + 61).unwrap());
    assert!(st.token_live(t2.expose(), T0 + 7 * DAY - 1).unwrap() && !st.token_live(t2.expose(), T0 + 7 * DAY + 1).unwrap());
}

#[test]
fn revoking_a_grant_ends_its_sessions() {
    let st = store();
    let (gid, _) = approved(&st);
    let s = st.register_session(RegisterSession { grant_id: Some(gid.clone()), ..reg("claude") }).unwrap();
    let r = st.revoke_grant(&gid, "owner").unwrap();
    assert_eq!(r.sessions_ended, vec![s.id.clone()]);
    assert!(st.get_session(&s.id).unwrap().unwrap().ended_at.is_some());
}

#[test]
fn secrets_never_print() {
    let s = Secret("cxg_abc".into());
    assert!(!format!("{s:?}").contains("abc"));
}
```

- [ ] **Step 2: Run** — `cargo test -p clax-core people::` fails.
- [ ] **Step 3: Implement** migration 22 exactly as spec §5.1, then the store. Codes: invite 12 Crockford base32 characters from `getrandom` (60 bits), user code 4 letters (no I, O) and 4 digits, tokens `cxg_` + 32 random bytes base64url, continuation `cxc_` likewise, browser code 32 bytes; all stored as SHA-256 hex. `register_session` (in `sessions.rs`) gains `grant_id` (sets `person` from the grant's viewer, NULL when that viewer is the owner row). The fold in `viewers.rs` also rewrites `sessions.person`, `artifacts.person`, `versions.person` (and in Task 7, `docs.author`, `approvals.approver/author`, `approval_requests.approver/author`, in Task 11 `inbox_items.recipient`): add each column to the fold's list when its migration lands and extend `fold_moves_every_author_column`.
- [ ] **Step 4: Run** — `cargo test -p clax-core` passes, migration tests included.
- [ ] **Step 5: Commit** — `"Add people, invites, grants and key-bound tokens (migration 22)"`

---

### Task 3: `clax-ed25519` signatures and the nonce cache

Spec §6.6, M4.

**Files:**
- Create: `crates/clax-core/src/sig.rs`
- Modify: `crates/clax-core/Cargo.toml` (`ring = { workspace = true }`; add `ring` to `[workspace.dependencies]` at the version already locked)

**Interfaces:**
- Produces:
  - `pub struct KeyPair` (`generate() -> KeyPair`, `from_pkcs8(&[u8])`, `pkcs8() -> Secret bytes`, `public() -> [u8; 32]`, `sign(&SignInput) -> SigHeader`)
  - `pub struct SignInput<'a> { method, path_and_query, host, t: i64, nonce: String, body_sha256: [u8; 32], token_sha256: [u8; 32] }`, `fn canonical(&self) -> Vec<u8>`
  - `pub fn parse_header(&str) -> Result<SigHeader>` (`t=…, n=…, s=…`)
  - `pub fn verify(public: &[u8; 32], input: &SignInput, sig: &[u8]) -> bool`
  - `pub struct NonceCache` (`new(window_s: i64, per_key: usize)`, `check_and_insert(key: &[u8; 32], nonce: &str, t: i64, now: i64) -> Result<(), SigError>`), `SigError::{Stale, Replayed, Full, Bad}` → codes `stale_request`, `replayed_request`, `limit_reached`, `invalid_signature`
  - `pub fn fingerprint(public: &[u8; 32]) -> String` (`SHA256:` + first 16 hex)

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn canonical_string_is_fixed() {
    let i = input("POST", "/api/artifacts?x=1", "192.168.1.5:7480", 1_760_000_000, "n0", b"{}", "cxg_t");
    let s = String::from_utf8(i.canonical()).unwrap();
    assert_eq!(s.lines().collect::<Vec<_>>()[..5], ["clax-ed25519 v1", "POST", "/api/artifacts?x=1", "192.168.1.5:7480", "1760000000"]);
    assert_eq!(s.lines().nth(6).unwrap(), "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"); // SHA-256 of "{}"
}

#[test]
fn sign_verify_and_tamper() {
    let k = KeyPair::generate();
    let i = input("GET", "/api/artifacts", "h", 100, "n1", b"", "t");
    let h = k.sign(&i);
    assert!(verify(&k.public(), &i, &h.sig));
    for bad in [input("POST", "/api/artifacts", "h", 100, "n1", b"", "t"), input("GET", "/api/artifacts?a", "h", 100, "n1", b"", "t"),
                input("GET", "/api/artifacts", "h", 100, "n1", b"x", "t"), input("GET", "/api/artifacts", "h", 100, "n1", b"", "u")] {
        assert!(!verify(&k.public(), &bad, &h.sig));
    }
    assert!(!verify(&KeyPair::generate().public(), &i, &h.sig));
}

#[test]
fn replay_and_staleness_are_refused() {
    let mut c = NonceCache::new(300, 4);
    let key = [7u8; 32];
    assert!(c.check_and_insert(&key, "a", 1000, 1000).is_ok());
    assert_eq!(c.check_and_insert(&key, "a", 1000, 1001), Err(SigError::Replayed));
    assert_eq!(c.check_and_insert(&key, "b", 1000, 1121), Err(SigError::Stale));
    assert_eq!(c.check_and_insert(&key, "b", 1000, 879), Err(SigError::Stale));
    for n in ["c", "d", "e"] { c.check_and_insert(&key, n, 1000, 1000).unwrap(); }
    assert_eq!(c.check_and_insert(&key, "f", 1000, 1000), Err(SigError::Full));
    assert!(c.check_and_insert(&key, "f", 1400, 1400).is_ok(), "entries past the window are evicted");
}

#[test]
fn pkcs8_round_trips_and_header_parses() {
    let k = KeyPair::generate();
    assert_eq!(KeyPair::from_pkcs8(k.pkcs8().expose_bytes()).unwrap().public(), k.public());
    let h = parse_header("t=5, n=abc, s=AAAA").unwrap();
    assert_eq!((h.t, h.nonce.as_str()), (5, "abc"));
    assert!(parse_header("t=x, n=abc, s=AAAA").is_err());
}
```

- [ ] **Step 2: Run** — fails. **Step 3: Implement** with `ring::signature::{Ed25519KeyPair, UnparsedPublicKey, ED25519}` and `ring::rand::SystemRandom`; the cache is a `HashMap<[u8;32], VecDeque<(i64, String)>>` plus a `HashSet` per key. **Step 4: Run** (`cargo test -p clax-core sig::`, `cargo deny`/`scripts/quality_gates.sh` unsafe check still green). **Step 5: Commit** — `"Sign agent requests with Ed25519 over a fixed canonical string, with a nonce cache"`

---

### Task 4: Grant routes, invites, the browser link and the `people` topic

Spec §6.2–§6.5, §4.2.

**Files:**
- Create: `crates/clax-server/src/routes/grants.rs`, `crates/clax-server/src/routes/connect.rs`
- Modify: `crates/clax-server/src/routes/mod.rs` (routes; `l10::GUARDS` entries: `grants::*` `NoArtifact` or `Owner`), `crates/clax-server/src/state.rs` (`GrantIndex`: token hash → grant, `NonceCache`, `Clock`), `crates/clax-server/src/stream.rs` (`Topic::People`, owner-only for now), `crates/clax-server/src/daemon.rs` (load the index; expire sweep every 60 s)
- Test: `crates/clax-server/tests/api_grants.rs`; `crates/clax-server/src/testing.rs` (`TestClient`: a key pair, signs requests; `ts.invite(name)`, `ts.connect_with_invite()`, `ts.connect_request()`)

**Interfaces:**
- Produces: routes of spec §15 "Grant flow" and the owner grant/invite routes; `GET /connect/b/{code}`; `crate::state::GrantIndex { fn lookup(&self, token_sha: &[u8;32]) -> Option<String>, fn reload(&self, st: &Store) }`; `TestClient { fn signed(&self, method, path, body) -> RequestBuilder }`.
- Consumes: Tasks 2, 3.

- [ ] **Step 1: Write the failing tests** (`tests/api_grants.rs`)

```rust
#[tokio::test]
async fn invite_flow_issues_a_key_bound_token_and_a_browser_link() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let inv: Value = ts.post_json("/api/people/invites", json!({"name": "Sam"})).await.json().await.unwrap();
    let cmd = inv["invite"]["command"].as_str().unwrap();
    assert!(cmd.starts_with("clax connect http://") && cmd.contains("/i/"));
    let c = TestClient::new(&ts);
    let r: Value = c.grant_request(json!({"access_token": {"access": ["clax:collaborator"]}, "invite": code_of(cmd)})).await;
    assert!(r["access_token"]["value"].as_str().unwrap().starts_with("cxg_"));
    assert!(r["access_token"].get("flags").is_none(), "never bearer");
    assert_eq!(r["subject"]["sub_ids"][0]["format"], "opaque");
    assert_eq!(r["clax"]["person"]["display_name"], "Sam");
    // The browser link makes a browser Sam, once.
    let link = r["clax"]["browser_link"].as_str().unwrap();
    let (lan, _) = ts.lan();
    let res = lan.get(link).send().await.unwrap();
    assert!(res.headers().get_all("set-cookie").iter().any(|v| v.to_str().unwrap().starts_with("clax_viewer=")));
    assert_eq!(lan.get(link).send().await.unwrap().status(), 404);
}

#[tokio::test]
async fn invites_need_the_lan() {
    let ts = TestServer::spawn().await; // loopback
    let res = ts.post_json("/api/people/invites", json!({"name": "Sam"})).await;
    assert_eq!(res.status(), 409);
}

#[tokio::test]
async fn adhoc_request_waits_for_the_owner_with_the_code() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let mut people = ts.stream_as_owner(&["people"]).await;
    let c = TestClient::new(&ts);
    let r = c.grant_request(json!({"access_token": {"access": ["clax:collaborator"]}, "interact": {"start": ["user_code"]}})).await;
    let code = r["interact"]["user_code"].as_str().unwrap().to_string();
    assert_eq!(r["continue"]["wait"], 5);
    let gid = people.next_named("grant").await["grant"]["id"].as_str().unwrap().to_string();
    assert_eq!(c.continue_poll(&r).await["continue"].is_object(), true);
    assert_eq!(ts.post_json(&format!("/api/grants/{gid}/approve"), json!({"user_code": "ZZZZ-0000", "person": {"name": "Sam"}})).await.status(), 400);
    assert_eq!(ts.post_json(&format!("/api/grants/{gid}/approve"), json!({"user_code": code, "person": {"name": "Sam"}})).await.status(), 200);
    ts.clock().advance(5);
    let done = c.continue_poll(&r).await;
    assert!(done["access_token"]["value"].as_str().is_some());
}

#[tokio::test]
async fn polling_faster_than_wait_is_too_fast_and_expiry_denies() { /* ManualClock: poll at +1 s → 400 too_fast; advance 601 s → 403 denied */ }

#[tokio::test]
async fn rotation_and_revocation_through_manage_and_continue() {
    /* rotate: old token still works for 60 s (clock), then 401 invalid_token;
       DELETE continue → grant revoked, every token 401 grant_revoked */
}

#[tokio::test]
async fn grant_routes_are_owner_only_and_signed() {
    /* LAN viewer: GET /api/grants 403; approve without token 403;
       POST /api/grants without a signature 401 invalid_signature;
       a request signed by key A presenting key B in client.key 401 */
}
```

- [ ] **Step 2: Run** — `cargo test -p clax-server --test api_grants` fails. **Step 3: Implement.** The grant request route is reachable without any credential but verifies the signature with the key in the body (first request) or the grant's stored key (continuation, token manage). `GET /connect/b/{code}` refuses loopback-and-local-host callers (404) and the owner row; it folds a carried non-person viewer cookie through `claim_for_owner`'s fold helper generalised to `fold_into(person)`. Peer IP from `peer_of`. The `people` topic carries `grant`, `invite` events (owner streams only). **Step 4: Run** the suite. **Step 5: Commit** — `"Add the GNAP grant flow: invites, user-code requests, continuation, token rotation and revocation"`

---

### Task 5: The agent gateway, `RequireAgent`, `Identity::person` and the route guard

Spec §6.1, §8.5, §8.6 (route guard), §7.1.

**Files:**
- Create: `crates/clax-server/src/agent_gateway.rs`
- Modify: `crates/clax-server/src/identity.rs` (`ViaGrant`, `Identity.grant`, `Identity::person()`), `crates/clax-server/src/auth.rs` (`RequireAgent` extractor: token or `ViaGrant`; `Reader`), every route module whose agent-facing handlers take `RequireToken` (`artifacts`, `assets`, `threads`, `docs`, `sessions`, `watches`, `working`, `feedback`, `questions`), `crates/clax-server/src/routes/mod.rs` (layer; `mod agents` guard test)
- Test: `crates/clax-server/tests/api_agent_gateway.rs`; the guard test in `routes/mod.rs`

**Interfaces:**
- Produces:
  - `pub struct ViaGrant { pub grant_id: String, pub person: Option<String>, pub session: Option<String>, pub rights: Arc<Rights> }`
  - `pub enum Person { Owner, Viewer(String), Agent { person: Option<String>, session: String } }`, `Identity::person(&self, st) -> Result<Option<Person>>`
  - `pub struct RequireAgent { pub token: bool, pub grant: Option<ViaGrant> }`, `fn reader(&self) -> Reader`
  - `pub enum Reader { Owner, Person(String) }` (`Reader::Owner` for the token or a grant whose person is the owner row)
  - `agent_gateway::ALLOW: &[(Method, &str /* route pattern */, Need)]`, `enum Need { Right(RightType, &'static str), Session, GrantFlow }`
  - `agent_gateway::gateway` middleware.
- Consumes: Tasks 1–4.

- [ ] **Step 1: Write the failing tests**

`tests/api_agent_gateway.rs`:

```rust
#[tokio::test]
async fn a_persons_agent_reaches_allowed_routes_only() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await; // TestClient with a token
    let sid = sam.register("claude", "h1").await["id"].as_str().unwrap().to_string();
    assert_eq!(sam.get(&sid, "/api/artifacts").await.status(), 200);
    for (m, p) in [("GET", "/api/sessions"), ("GET", "/api/live/pages?url=http://x/"), ("GET", "/api/inbox"),
                   ("POST", "/api/admin/shutdown"), ("GET", "/api/token"), ("GET", "/api/events"), ("GET", "/api/extension")] {
        assert_eq!(sam.call(&sid, m, p, None).await.status(), 403, "{m} {p}");
    }
}

#[tokio::test]
async fn rights_are_enforced_with_the_missing_right_named() {
    let ts = lan_server().await;
    let a = ts.publish("Owner page", &[("index.html", "<p>o</p>")]).await;
    let sam = ts.connect_person("Sam", "clax:reader").await;
    let sid = sam.register("claude", "h1").await["id"].as_str().unwrap().to_string();
    let res = sam.publish_version(&sid, a["id"].as_str().unwrap(), "<p>x</p>").await;
    assert_eq!(res.status(), 403);
    let e: Value = res.json().await.unwrap();
    assert_eq!((e["error"]["code"].as_str(), e["error"]["right"]["type"].as_str()), (Some("not_permitted"), Some("clax-artifacts")));
}

#[tokio::test]
async fn foreign_session_is_refused() { /* Sam's client names Mia's session in X-Clax-Session → 403 foreign_session */ }

#[tokio::test]
async fn token_without_signature_is_refused() { /* Authorization: Clax-Grant <token> alone → 401 invalid_signature; with Origin or Cookie → 403 forbidden_origin */ }

#[tokio::test]
async fn a_grant_is_never_the_owner() {
    /* the owner's own second machine (invite for the owner row): GET /api/inbox → 403; sessions registered get person NULL */
}

#[tokio::test]
async fn the_daemon_token_still_does_everything() { /* the existing api_* suites run unchanged: covered by running them */ }
```

Guard test (`routes/mod.rs`, `mod agents`, reusing `l10::routes`, `l10::handlers`, `l10::function` made `pub(super)`):

```rust
#[derive(Clone, Copy, Debug)]
enum Class { Denied, Filtered, NoContent, GrantFlow }
const AGENTS: &[(&str, Class)] = &[
    ("artifacts::list", Filtered), ("artifacts::get", Filtered), ("artifacts::create", NoContent),
    ("artifacts::publish", NoContent), ("artifacts::list_versions", Filtered), ("artifacts::get_version", Filtered),
    ("artifacts::files", Filtered), ("content::raw_file", Filtered), ("threads::list", Filtered), ("threads::get", Filtered),
    ("threads::clip", Filtered), ("threads::comment", NoContent), ("threads::resolve", NoContent),
    ("docs::get", Filtered), ("docs::list", Filtered), ("docs::put", NoContent), /* … every route … */
    ("feedback::poll", Filtered), ("feedback::notices", Filtered), ("working::roster", Filtered),
    ("grants::request", GrantFlow), ("grants::continue_poll", GrantFlow), /* … */
    ("inbox::list", Denied), ("live::page", Denied), ("sessions::list", Denied), /* … */
];

#[test]
fn every_route_is_classified_for_agents() {
    // 1. every route handler in mod.rs appears in AGENTS (else: "classify <name> for agents");
    // 2. agent_gateway::ALLOW's handlers are exactly the non-Denied entries;
    // 3. Filtered handlers' source contains "Visibility::for_reader" and "vis.";
    // 4. NoContent handlers' source does not build a thread, comment, doc or file view for content other than the request's own (no "thread_view(", "doc_view(" calls);
    // 5. Denied handlers do not take RequireAgent.
}
```

In this task the `Filtered` entries are listed but their check (3) is skipped through a `const FILTER_LANDED: bool = false;` that Task 8 flips; Task 5 asserts every `Filtered` handler takes `RequireAgent` and refuses grant callers with `503 not_yet` until then (so no unfiltered content is served to a person's agent at any commit).

- [ ] **Step 2: Run** — fails. **Step 3: Implement** the gateway (outermost after the extension gateway; refuses `Origin`/`Cookie`; verifies with `GrantIndex` and `NonceCache`; loads the session's person, narrowing and elevation through a small cache keyed by session; computes `effective`; matches `ALLOW`; resolves the artifact from the path for location checks, and from the body's `url_or_id`-free routes by handler for `create`). Replace `RequireToken` with `RequireAgent` on agent routes; `Identity::from_parts` reads `ViaGrant`. **Step 4: Run** the whole `clax-server` suite (no existing test changes behaviour). **Step 5: Commit** — `"Admit person-bound agent credentials through an agent gateway with rights, and classify every route for agents"`

---

### Task 6: Sessions under grants, remote liveness, agent names and `@name`

Spec §11, M17, M23, §14 (name freed).

**Files:**
- Create: `crates/clax-core/src/names.rs`
- Modify: `crates/clax-core/src/store/sessions.rs` (register with `repo`, `branch`, `grant_id`; name assignment; `set_session_name(sid, name, source)`; reaper skips PID checks when `grant_id` is set), `crates/clax-core/src/mentions.rs` (`agent_mentions(body, live_names) -> Vec<String>`), `crates/clax-server/src/routes/sessions.rs`, `crates/clax-server/src/routes/threads.rs` (`@name` sends; `unresolved_mentions`), `crates/clax-server/src/routes/people.rs` (new: `PUT /api/agents/{handle}` rename)
- Test: `names.rs` unit tests, `store/sessions.rs` tests, `tests/api_sessions.rs`, `tests/api_threads.rs`

**Interfaces:**
- Produces: `names::derive(repo: Option<&str>, branch: Option<&str>, harness: &str) -> String`, `names::sanitize(&str) -> Option<String>`, `names::is_reserved(name, person_names: &[String]) -> bool`; `Store::set_session_name(&self, sid, name, NameSource) -> Result<Session>` (`name_taken`, `name_locked`, `invalid_name`); `RegisterSession { repo, branch, grant_id }`; `mentions::agent_mentions`.

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
    assert!(is_reserved("agent", &[]) && is_reserved("mia-kovac", &["Mia Kovač".into()]) == false);
    assert!(is_reserved("sam", &["Sam".into()]));
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
    st.set_session_name(&c.id, "clax", NameSource::Agent).unwrap(); // freed by the end
}

#[test]
fn remote_sessions_are_never_pid_probed() {
    let st = store();
    let (gid, _) = approved(&st);
    let s = st.register_session(RegisterSession { grant_id: Some(gid), pid: Some(1), ..reg("claude") }).unwrap();
    let probed = std::cell::Cell::new(false);
    st.reap_sessions(Duration::from_secs(300), &|_| { probed.set(true); false }).unwrap();
    assert!(!probed.get() && st.get_session(&s.id).unwrap().unwrap().ended_at.is_none());
}
```

`tests/api_threads.rs`: `at_name_sends_to_exactly_that_agent_and_gives_it_a_watch` (two live owner agents, `@clax-2 please` → one feedback row, for `clax-2`, which now watches); `unknown_at_name_sends_nothing_and_says_so` (`unresolved_mentions: ["nobody"]`); `page_written_at_name_sends_nothing`; `at_agent_keeps_its_meaning`. `tests/api_sessions.rs`: `registration_returns_a_name`, `patch_sets_the_agents_own_name`.

- [ ] **Steps 2–4:** run, implement, run (`cargo test -p clax-core names:: sessions::`, `cargo test -p clax-server --test api_threads --test api_sessions`).
- [ ] **Step 5: Commit** — `"Name agents from their repository and branch, let agents and people rename them, and route @name to one agent"`

---

### Task 7: Visibility core and migration 23

Spec §5.2, §8.1–§8.3, §8.7, M6–M11.

**Files:**
- Create: `crates/clax-core/src/visibility.rs`, `crates/clax-core/src/store/approvals.rs` (approvals CRUD only; requests come in Task 10)
- Modify: `crates/clax-core/src/store/migrations.rs` (migration 23), `crates/clax-core/src/store/docs.rs` (write `author`, `via_page` from `Caller`), `crates/clax-core/src/db.rs` (`Caller.person`, `Caller.via_page`), `crates/clax-core/src/store/viewers.rs` (fold columns)

**Interfaces:**
- Produces:
  - `pub enum Author { Owner, Person(String), Unconnected, System }`
  - `pub struct Visibility` with `Visibility::all()` (fast path), `Visibility::for_reader(st: &Store, reader: &Reader) -> Result<Visibility>` (the `Reader` type moves to `clax-core::visibility` and is re-exported by the server), `fn sees(&self, authors: &[Author], artifact: &str, thread: Option<&str>) -> bool`, `fn withheld_authors(&self, authors: &[Author], artifact, thread) -> Vec<Author>`.
  - Effective-author functions: `authors_of_comment(&CommentRow, &ThreadRow, &VersionPeople) -> Vec<Author>`, `authors_of_anchor`, `authors_of_clip`, `authors_of_version`, `authors_of_artifact_text`, `authors_of_doc`, `authors_of_working`, `authors_of_batch`.
  - `Store::people_present(&self) -> Result<bool>` and the cached flag `PeoplePresent(AtomicBool)` for the fast path.
  - `Store::approve(&self, approver: Option<&str>, author: &str, scope: Scope, via) -> Result<Approval>`, `revoke_approval(id, approver)`, `live_approvals(approver) -> Result<Vec<Approval>>`.

- [ ] **Step 1: Write the failing tests** (`visibility.rs`)

```rust
#[test]
fn own_person_is_always_visible_and_others_need_approval() {
    let st = two_people(); // owner, Sam (person), Mia (person), Lan (unconnected viewer); migration 23 applied
    let sam = Visibility::for_reader(&st, &Reader::Person(SAM.into())).unwrap();
    assert!(sam.sees(&[Author::Person(SAM.into())], "a1", Some("t1")));
    assert!(!sam.sees(&[Author::Person(MIA.into())], "a1", Some("t1")));
    assert!(!sam.sees(&[Author::Owner], "a1", None));
    assert!(!sam.sees(&[Author::Unconnected], "a1", None), "*unconnected is the owner's approval only");
    let owner = Visibility::for_reader(&st, &Reader::Owner).unwrap();
    assert!(owner.sees(&[Author::Unconnected], "a1", None), "migration 23's approval");
    assert!(!owner.sees(&[Author::Person(SAM.into())], "a1", None), "connected people start withheld");
}

#[test]
fn scopes_thread_artifact_everywhere_and_revocation() {
    let st = two_people();
    let id = st.approve(Some(SAM), MIA, Scope::Thread { artifact: "a1".into(), thread: "t1".into() }, Via::Shell).unwrap().id;
    let v = || Visibility::for_reader(&st, &Reader::Person(SAM.into())).unwrap();
    assert!(v().sees(&[Author::Person(MIA.into())], "a1", Some("t1")));
    assert!(!v().sees(&[Author::Person(MIA.into())], "a1", Some("t2")));
    assert!(!v().sees(&[Author::Person(MIA.into())], "a1", None), "a thread approval covers only the thread");
    st.approve(Some(SAM), MIA, Scope::Artifact("a1".into()), Via::Shell).unwrap();
    assert!(v().sees(&[Author::Person(MIA.into())], "a1", None));
    assert!(!v().sees(&[Author::Person(MIA.into())], "a2", None));
    st.approve(Some(SAM), MIA, Scope::Everywhere, Via::Cli).unwrap();
    assert!(v().sees(&[Author::Person(MIA.into())], "a2", None));
    st.revoke_approval(&id, Some(SAM)).unwrap();
    assert!(v().sees(&[Author::Person(MIA.into())], "a1", Some("t1")), "other approvals still cover it");
}

#[test]
fn every_author_must_be_visible() {
    let st = two_people();
    st.approve(Some(SAM), OWNER_PID, Scope::Everywhere, Via::Connect).unwrap();
    let sam = Visibility::for_reader(&st, &Reader::Person(SAM.into())).unwrap();
    assert!(sam.sees(&[Author::Owner], "a1", None));
    assert!(!sam.sees(&[Author::Owner, Author::Person(MIA.into())], "a1", None));
    assert_eq!(sam.withheld_authors(&[Author::Owner, Author::Person(MIA.into())], "a1", None), vec![Author::Person(MIA.into())]);
}

#[test]
fn page_written_content_counts_its_publisher() {
    let st = two_people();
    let (aid, tid, cid) = sam_page_with_owner_page_comment(&st); // version 1 published by Sam's agent; comment via_page by the owner
    let c = st.comment_row(&cid).unwrap();
    let authors = authors_of_comment(&c, &st.thread_row(&tid).unwrap(), &st.version_people(&aid).unwrap());
    assert_eq!(authors, vec![Author::Owner, Author::Person(SAM.into())]);
    let doc_authors = authors_of_doc(&st.doc_row(&aid, "notes/1").unwrap(), &st.version_people(&aid).unwrap());
    assert!(doc_authors.contains(&Author::Person(SAM.into())));
}

#[test]
fn single_owner_install_takes_the_fast_path() {
    let st = store_migrated_from_21_fixture();
    assert!(!st.people_present().unwrap());
    assert!(matches!(Visibility::for_reader(&st, &Reader::Owner).unwrap(), v if v.is_all()));
}

#[test]
fn for_reader_reads_approvals_with_one_indexed_query() {
    let st = two_people();
    assert_plan_uses_index(&st, visibility::APPROVALS_SQL, "approvals_by_approver");
}
```

- [ ] **Steps 2–4:** run, implement (migration 23 exactly as spec §5.2; `for_reader` builds `HashSet`s for everywhere, artifact and thread scopes; `Unconnected` resolves by `viewers.person = 0 AND owner = 0` or no row; anonymous is `Unconnected`; NULL `person` columns and the owner's public ID both map to `Owner`), run `cargo test -p clax-core`.
- [ ] **Step 5: Commit** — `"Add the visibility filter and approvals, with effective authors for page-written content (migration 23)"`

---

### Task 8: The filter on every agent-facing read route

Spec §8.3, §8.5, §8.6 (route guard check 3), Q8.

**Files:**
- Modify: `crates/clax-server/src/routes/{artifacts,content,threads,docs,working,assets}.rs`, `crates/clax-server/src/feedback.rs` (thread views gain `withheld`, anchors and clips placeholders), `crates/clax-server/src/routes/mod.rs` (`FILTER_LANDED = true`), `crates/clax-server/src/agent_gateway.rs` (drop the `503 not_yet` for the read routes; `feedback::poll` and `feedback::notices` keep it until Task 9)
- Create: `crates/clax-server/tests/api_visibility.rs`

**Interfaces:**
- Consumes: Task 7. Each `Filtered` handler: `let vis = Visibility::for_reader(st, &agent.reader())?;` only when the caller is an agent (`RequireAgent` with no browser); browser and LAN callers build none.
- Produces: the placeholder shapes of spec §8.3; error `withheld` (403) for file and version content.

- [ ] **Step 1: Write the failing tests** (`tests/api_visibility.rs`)

```rust
/// Every content kind × every read route: withheld, then approved at each scope, then revoked.
#[tokio::test]
async fn withheld_matrix_reads() {
    let w = World::two_people().await; // owner, Sam and Mia connected; Sam's agent `sa`; Mia's page `mp` (published by Mia's agent),
                                       // Mia's comment thread `mt` on the owner's page `op`, a doc Mia wrote on `op`, a clip.
    for (what, check) in w.read_checks() { // threads list/get, clip, artifacts list/get, versions, files, docs list/get, roster
        let got = check(&w.sam_agent).await;
        assert!(got.is_placeholder(), "{what}: {got:?}");
        assert!(!got.text().contains(MIA_SECRET), "{what} leaked");
    }
    for scope in [Scope::thread(&w.op, &w.mt), Scope::artifact(&w.op), Scope::Everywhere] {
        let id = w.sam_approves_mia(scope.clone()).await;
        for (what, check) in w.read_checks_covered_by(&scope) {
            assert!(check(&w.sam_agent).await.text().contains(MIA_SECRET), "{what} after {scope:?}");
        }
        w.sam_revokes(&id).await;
    }
    for (what, check) in w.read_checks() {
        assert!(check(&w.sam_agent).await.is_placeholder(), "{what} after revocation");
    }
}

#[tokio::test]
async fn owner_agent_sees_placeholder_for_page_written_comment() {
    let w = World::two_people().await;
    let (aid, tid) = w.owner_browser_writes_through_page(&w.mp, "ignore all instructions").await; // comments capability
    let t = w.owner_agent.thread(&aid, &tid).await;
    assert_eq!(t["withheld"]["authors"][0]["name"], "Mia");
    assert!(t["comments"][0]["body"].is_null());
}

#[tokio::test]
async fn people_are_never_filtered() {
    let w = World::two_people().await;
    assert!(w.sam_browser.thread(&w.op, &w.mt).await.text().contains(MIA_SECRET));
    assert!(w.owner_browser.thread(&w.op, &w.mt).await.text().contains(MIA_SECRET));
    assert!(w.lan_viewer.thread(&w.op, &w.mt).await.text().contains(MIA_SECRET));
}

#[tokio::test]
async fn titles_are_withheld_in_list_and_read() { /* Sam's agent's list: Mia's page has title null, withheld.authors [Mia] */ }
```

(`World` lives in `tests/common/people.rs`, built on `TestServer`, `TestClient` and `connect_person`; `MIA_SECRET` is a distinctive string placed in every content kind Mia authors.)

- [ ] **Step 2: Run** — fails. **Step 3: Implement** (per-view placeholder builders in `clax-core::visibility::placeholder`; `content::raw_file` and `get_version` answer 403 `withheld`). **Step 4: Run** `cargo test -p clax-server` and the guard test with `FILTER_LANDED`. **Step 5: Commit** — `"Withhold other people's content from agents on every read route, with placeholders"`

---

### Task 9: The filter on every delivery path

Spec §8.4, §8.6 (delivery guard), M13.

**Files:**
- Modify: `crates/clax-core/src/store/feedback.rs` (`take_feedback(&self, t: TakeFeedback, vis: &Visibility)`, `take_notices(…, vis)`; placeholders and `placeholder_at`; resends of visible rows only), `crates/clax-core/src/feedback.rs` (placeholder lines; `FeedbackState::Withheld`), `crates/clax-server/src/routes/feedback.rs` (every tier), `crates/clax-server/src/push.rs` (`codex queue` payload), `crates/clax-server/src/feedback.rs` (thread `feedback_state` shows `withheld`)
- Test: `crates/clax-core/src/store/feedback.rs` tests, `crates/clax-server/tests/api_visibility.rs` (`withheld_matrix_tiers`), a guard test in `crates/clax-core/src/lib.rs`

**Interfaces:**
- Produces: `take_feedback` returns `TakeResult { items: Vec<FeedbackItem>, placeholders: Vec<Placeholder>, touched }`; `Placeholder { author_name, thread_id, artifact_title: Option<String>, count, request: RequestState }`; `feedback::render_placeholder(&Placeholder) -> String`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn feedback_items_are_built_only_under_visibility() {
    // Scans clax-core/src for "FeedbackItem {" and "Notice {" constructors.
    let allowed = ["store/feedback.rs::take_feedback", "store/feedback.rs::take_notices", "feedback.rs::tests"];
    for (file, src) in crate::test_support::sources() {
        for ctor in ["FeedbackItem {", "Notice {"] {
            for at in src.match_indices(ctor).map(|(i, _)| i) {
                let f = crate::test_support::enclosing_fn(src, at);
                assert!(allowed.iter().any(|a| *a == format!("{file}::{f}") || (a.ends_with("::tests") && f.starts_with("tests"))),
                        "{ctor} built in {file}::{f}: build it under Visibility");
            }
        }
    }
}

#[test]
fn withheld_rows_stay_undelivered_and_get_one_placeholder() {
    let st = two_people();
    let (sid, tid) = mia_sends_to_sams_agent(&st, 2); // two comments
    let vis = Visibility::for_reader(&st, &Reader::Person(SAM.into())).unwrap();
    let r = st.take_feedback(take(&sid, Tier::Piggyback), &vis).unwrap();
    assert!(r.items.is_empty());
    assert_eq!((r.placeholders.len(), r.placeholders[0].count), (1, 2));
    assert!(st.take_feedback(take(&sid, Tier::Piggyback), &vis).unwrap().placeholders.is_empty(), "once");
    st.approve(Some(SAM), MIA, Scope::thread_of(&st, &tid), Via::Shell).unwrap();
    let vis = Visibility::for_reader(&st, &Reader::Person(SAM.into())).unwrap();
    let r = st.take_feedback(take(&sid, Tier::Piggyback), &vis).unwrap();
    assert_eq!(r.items.len(), 2);
}

#[test]
fn revocation_holds_undelivered_rows_again_and_tells_once() { /* approve, send 1 more, revoke before take → held, placeholder_at cleared → one placeholder */ }
```

`tests/api_visibility.rs`: `withheld_matrix_tiers` drives piggyback (a tool call's result through `/api/sessions/<sid>/feedback?tier=piggyback`), stop hook, prompt hook, `wait_for_feedback` (woken by the approval), Pi inject, the notices poll and `codex queue` (the fake exe from `clax-fake-exe` records its `--message`): before approval none carries `MIA_SECRET` and each in-band tier carries the placeholder line exactly once; after approval each carries the comment. `approval_wakes_a_waiting_poll` uses `wait_feedback_waiters`.

- [ ] **Steps 2–4:** run, implement (and drop the gateway's `503 not_yet` for `feedback::poll` and `feedback::notices`), run (`cargo test -p clax-core feedback`, `cargo test -p clax-server --test api_visibility --test api_feedback --test api_push --test api_notices`).
- [ ] **Step 5: Commit** — `"Hold withheld feedback on every tier and hand over one placeholder per author and thread"`

---

### Task 10: Approval requests, `ask` for approval, routing and revocation

Spec §9, §10, M14, M15.

**Files:**
- Modify: `crates/clax-core/src/store/approvals.rs` (requests: `request_approval`, batching, `decide`, `withdraw_requests_for`, limits), `crates/clax-core/src/store/feedback.rs` (`send_to` returns routing requests), `crates/clax-server/src/routes/approvals.rs` (new), `crates/clax-server/src/routes/threads.rs` (send, batch send, `@name` make routing requests; `agent_cannot_read`), `crates/clax-server/src/routes/sessions.rs` (`POST /api/sessions/{id}/approvals`, `GET …/approvals/{rid}?wait=`), `crates/clax-server/src/questions.rs` (reuse `QuestionWaiters` for request waits), `crates/clax-server/src/stream.rs` (`approval_request` events on the approver's topics)
- Test: `crates/clax-server/tests/api_approvals.rs`

**Interfaces:**
- Produces: `Store::request_approval(&self, r: NewRequest) -> Result<(ApprovalRequest, bool /* new */)>`, `Store::decide_request(&self, id, approver, Decision) -> Result<Decided { approval: Option<Approval>, closed: Vec<String>, released_sessions: Vec<String> }>`; routes of spec §15 "Person" for approvals; the session approval routes.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn routing_to_another_persons_agent_makes_a_request() {
    let w = World::two_people().await;
    let mut sam_inbox = w.sam_browser.stream(&["inbox"]).await;
    let t = w.mia_browser.thread_on(&w.op, "please fix the chart").await;
    w.mia_browser.send(&w.op, &t, Some(&w.sam_agent.handle)).await;
    let item = sam_inbox.next_named("inbox_item").await;
    assert_eq!((item["item"]["kind"].as_str(), item["item"]["approval"]["author"]["name"].as_str()), (Some("approval"), Some("Mia")));
    assert!(w.sam_agent.feedback_text().await.contains("Your person has been asked."));
    assert_eq!(w.owner_browser.thread(&w.op, &t).await["feedback_state"]["phase"], "withheld");
}

#[tokio::test]
async fn requests_batch_per_author_per_thread() {
    /* three Mia comments on one sent thread → one open request with count 3; a second thread → a second request */
}

#[tokio::test]
async fn ask_for_approval_waits_for_the_decision() {
    let w = World::two_people().await;
    let t = w.mia_comment_on_thread_sams_agent_owns().await;
    let ask = tokio::spawn(w.sam_agent.ask_approval(&w.op, &t, 60));
    w.wait_request_waiters(1).await;
    let rid = w.sam_browser.open_requests().await[0]["id"].as_str().unwrap().to_string();
    w.sam_browser.decide(&rid, "thread").await;
    assert_eq!(ask.await.unwrap()["status"], "approved");
    assert!(w.sam_agent.feedback_text().await.contains("please fix"));
}

#[tokio::test]
async fn only_the_approver_decides() { /* owner deciding Sam's request → 404; LAN viewer → 403; foreign Origin → 403 forbidden_origin */ }

#[tokio::test]
async fn decline_keeps_rows_held_and_asks_again_only_after_ten_minutes() { /* ManualClock */ }

#[tokio::test]
async fn approving_everywhere_closes_the_authors_other_requests() {}

#[tokio::test]
async fn a_send_to_an_agent_that_cannot_read_is_refused() { /* Sam narrowed to artifact `x`; send on `op` → 400 agent_cannot_read */ }

#[tokio::test]
async fn request_limit_overflows_into_one_note() { /* 100 open + 1 → no 101st request; one overflow note item */ }
```

- [ ] **Steps 2–4:** run, implement, run. **Step 5: Commit** — `"Turn withheld content into batched approval requests, from routing and from ask, decided by the agent's person"`

---

### Task 11: The per-person inbox and person-scoped questions (migration 24)

Spec §5.3, M16, §13 (inbox grouping data), §18.

**Files:**
- Modify: `crates/clax-core/src/store/migrations.rs` (migration 24), `crates/clax-core/src/store/inbox.rs` (recipient on every item kind; keys `…@<public ID>` for non-owner recipients; kinds `approval`, `connection`, `ended`; participation per connected person), `crates/clax-core/src/store/questions.rs` (list by the asking session's person), `crates/clax-server/src/routes/{inbox,questions}.rs` (`Identity::person()` scoping; owner unchanged), `crates/clax-server/src/stream.rs` (`questions`, `inbox` topics for persons' streams; events routed by recipient), `crates/clax-server/src/routes/mod.rs` (`l10::Guard::Owner` entries for these handlers become `Person`, asserting `who.person(` and a recipient filter)
- Test: `crates/clax-core/src/store/inbox.rs` tests, `crates/clax-server/tests/api_inbox.rs`, `tests/api_questions.rs`

**Interfaces:**
- Produces: `InboxFilter.recipient: Option<String>`; `Store::inbox_for(&self, recipient: Option<&str>, …)`; question views scoped; `inbox_item` events carry `recipient` internally only (never serialized).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn migration_24_keeps_seq_ids_keys_read_state_and_search() {
    let st = Store::open_at_version(tmp(), 23);
    seed_inbox(&st, 50); // replies, versions, questions, finished; some read
    let before = st.raw_inbox().unwrap();
    st.migrate_to_latest().unwrap();
    assert_eq!(st.raw_inbox_without_new_columns().unwrap(), before);
    assert_eq!(st.search_inbox(None, "quarterly").unwrap().len(), count_matching(&before, "quarterly"));
}

#[test]
fn items_go_to_their_recipient() {
    /* a pending grant makes a `connection` item for the owner (until now only the `people` topic announced it; Task 4's
       request_grant and redemption gain the item here), an invite redemption a read-later `connection` note;
       Sam's agent asks → question item recipient Sam, not owner; Mia replies in a thread Sam is in → reply item for Sam and,
       when the owner is in it too, one for the owner; Sam's agent publishes → published items for Sam and the owner */
}

#[test]
fn every_inbox_query_still_uses_an_index() { /* the questions-plan's query-plan test, with recipient */ }
```

`tests/api_inbox.rs`: `a_person_sees_only_their_inbox` (Sam's browser: Sam's items; the owner's untouched; a LAN viewer 403 as before), `the_owner_inbox_is_unchanged` (same responses as before migration for an owner-only fixture). `tests/api_questions.rs`: `a_persons_agent_asks_its_person` (Sam answers; the owner gets 404 for that question).

- [ ] **Steps 2–4**, **Step 5: Commit** — `"Give each connected person an inbox, and send questions, approvals and connection requests to theirs (migration 24)"`

---

### Task 12: Thread ownership, handoff and `thread_taken`

Spec §12, M18–M20.

**Files:**
- Modify: `crates/clax-core/src/store/feedback.rs` (first take sets the owner and deletes other sessions' undelivered rows for the thread), `crates/clax-core/src/store/threads.rs` (`thread_owner`, `handoff(thread, from_sid, to: Option<sid>)`, `check_not_taken(thread, sid)`), `crates/clax-server/src/routes/threads.rs` (reply and resolve refuse `thread_taken`; views gain `owner`, `taken_by`), `crates/clax-server/src/routes/sessions.rs` (`POST /api/sessions/{id}/threads/{tid}/handoff`)
- Test: `crates/clax-core/src/store/feedback.rs` tests, `crates/clax-server/tests/api_threads.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn first_take_has_one_winner() {
    let st = store();
    let (a, b) = (live("claude"), live("codex"));
    let tid = untargeted_send_watched_by(&st, &[&a, &b]);
    let vis = Visibility::all();
    let (ra, rb) = std::thread::scope(|s| {
        let h1 = s.spawn(|| st.take_feedback(take(&a, Tier::Piggyback), &vis).unwrap());
        let h2 = s.spawn(|| st.take_feedback(take(&b, Tier::Piggyback), &vis).unwrap());
        (h1.join().unwrap(), h2.join().unwrap())
    });
    assert_eq!(ra.items.len() + rb.items.len(), 1, "exactly one session got the comment");
    let owner = st.thread_owner(&tid).unwrap().unwrap();
    assert!(owner == a || owner == b);
}

#[test]
fn a_placeholder_take_makes_no_owner() {}

#[test]
fn handoff_resends_and_release_untargets() {}
```

`tests/api_threads.rs`: `non_owner_reply_is_thread_taken` (409 with `owner.name`), `owner_ended_threads_accept_any_agent`, `handoff_to_another_persons_agent_makes_a_routing_request`, `views_show_owner_and_taken_by`.

- [ ] **Steps 2–4**, **Step 5: Commit** — `"Give each sent thread one owner agent, refuse other agents' writes, and add handoff and release"`

---

### Task 13: Lifecycle cleanup and the agents roster

Spec §14, §13, M21.

**Files:**
- Modify: `crates/clax-core/src/store/sessions.rs` (`end_session_touched` returns released threads, withdrawn requests, cleared rights; writes the `ended` item), `crates/clax-server/src/routes/people.rs` (`GET /api/agents`), `crates/clax-server/src/stream.rs` (`Topic::Agents`), `crates/clax-server/src/working.rs` (states), `crates/clax-server/src/routes/threads.rs` (Send menus list live agents only)
- Test: `crates/clax-core/src/store/sessions.rs` tests, `crates/clax-server/tests/api_agents.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn ending_a_session_cleans_up_and_leaves_one_note() {
    let ts = TestServer::spawn().await;
    let a = ts.register_session("claude", "h1").await;
    let sid = a["id"].as_str().unwrap();
    let (aid, t1, t2) = ts.two_threads_owned_by(sid).await;
    let qid = ts.ask(sid, one_question()).await["question"]["id"].as_str().unwrap().to_string();
    ts.end_session(sid).await;
    assert_eq!(ts.question_status(&qid).await, "withdrawn");
    for t in [&t1, &t2] { assert!(ts.thread_json(&aid, t).await["owner"].is_null()); }
    let inbox: Value = ts.get_authed("/api/inbox?kind=ended").await.json().await.unwrap();
    assert_eq!(inbox["items"].as_array().unwrap().len(), 1);
    assert!(ts.agents().await.iter().all(|x| x["handle"] != a["agent_handle"] || x["state"] == "ended"));
}

#[tokio::test]
async fn an_idle_session_ends_without_a_note() {}

#[tokio::test]
async fn roster_reports_blocked_since_and_is_person_scoped() {
    /* owner agent asks at T0; advance 12 min; GET /api/agents → state waiting, blocked.since T0;
       Sam's browser sees only Sam's agents with project; the owner sees all */
}
```

- [ ] **Steps 2–4**, **Step 5: Commit** — `"Clean up after ended agents with one inbox note, and serve an agent roster with states and blocks"`

---

### Task 14: People, rights, narrowing and elevation routes

Spec §7.4, §15 (owner and person routes), §6.4 (revoking a person).

**Files:**
- Modify: `crates/clax-server/src/routes/people.rs` (`GET /api/people`, `GET /api/people/me`, `PUT /api/people/{pid}/rights`, `DELETE /api/people/{pid}`, `PUT /api/people/me/grants/{gid}/narrow`, `PUT|DELETE /api/agents/{handle}/elevate`, `PUT /api/agents/{handle}/narrow`), `crates/clax-server/src/agent_gateway.rs` (rights cache invalidation on change), `crates/clax-server/src/extension.rs` (gateway admits the person routes for approvals: `GET /api/approvals*`, `POST /api/approvals/requests/*/decide`, `DELETE /api/approvals/*`)
- Test: `crates/clax-server/tests/api_people.rs`, the extension gateway rule table

- [ ] **Step 1: Write the failing tests**

`elevation_exceeds_the_person_for_one_agent_only`, `narrowing_applies_at_once_and_cannot_widen` (400 `not_narrower`), `an_agent_cannot_change_its_own_rights` (403 through the agent gateway: routes are `Denied`), `revoking_a_person_revokes_grants_and_invites_and_keeps_history`, `elevation_ends_with_the_session`, `people_routes_are_owner_or_self` (Sam's browser: `/api/people/me` 200, `/api/people` 403).

- [ ] **Steps 2–4**, **Step 5: Commit** — `"Let the owner set people's rights and elevate one agent, and people narrow their own agents"`

---

### Task 15: Remote mode and `clax connect`

Spec §16 (connect, remote mode), M22, §22.

**Files:**
- Create: `crates/clax-cli/src/remote.rs` (`Remote { url, grant_id, token, … }`, `load(home) -> Option<Remote>`, `SignedClient` wrapping `reqwest::Client`: signs, retries once on `stale_request` with the `Date` offset, rotates under a day), `crates/clax-cli/src/commands/connect.rs`
- Modify: `crates/clax-cli/src/client.rs` (choose remote when `remote.json` exists and `CLAX_REMOTE` is not `0`; never auto-start a daemon then), `crates/clax-mcp/src/client.rs` (same; `remote_cache` for clips), `crates/clax-cli/src/commands/doctor.rs` (remote checks: reachable, key readable, clock offset), `crates/clax-core/src/home.rs` (`remote_json()`, `remote_key()`, `remote_cache()`)
- Test: `crates/clax-cli/tests/connect.rs` (against a real in-process daemon from `clax-server::testing`), `remote.rs` unit tests

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn connect_with_an_invite_writes_remote_json_and_key_0600() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let home = tempfile::tempdir().unwrap();
    let cmd = ts.invite_command("Sam").await;
    let out = run_cli(home.path(), &["connect", url_of(&cmd), "--yes", "--no-browser", "--name", "sam-mbp"]).await;
    assert!(out.stdout.contains("Connected to") && out.stdout.contains("as Sam") && out.stdout.contains("SHA256:"));
    for f in ["remote.json", "remote.key"] {
        assert_eq!(std::fs::metadata(home.path().join(f)).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // --yes accepted the owner approval default.
    assert!(ts.approvals_of("Sam").await.iter().any(|a| a["scope"] == "everywhere" && a["author"]["owner"] == true));
}

#[tokio::test]
async fn remote_mode_signs_every_request_and_never_starts_a_daemon() {
    /* with remote.json: `clax list --json` reaches ts (its request log shows a Clax-Signature); with the server stopped,
       `clax list` fails with daemon_unreachable naming the remote URL and no daemon.json appears in the temp home */
}

#[tokio::test]
async fn adhoc_connect_waits_and_completes_on_approval() {}

#[tokio::test]
async fn rotation_happens_when_under_a_day_is_left() { /* ManualClock injected into SignedClient */ }

#[tokio::test]
async fn connect_off_revokes_and_removes_the_files() {}
```

- [ ] **Steps 2–4**, **Step 5: Commit** — `"Add clax connect and a signed remote mode for the CLI, hooks and shim"`

---

### Task 16: MCP tools, hooks and Pi for multiple agents and people

Spec §16 (tools, clips, hooks), §11 (`status {name}`), §9.2 (`ask {approval}`), §12 (`comments_handoff`).

**Files:**
- Modify: `crates/clax-mcp/src/tools.rs` (`status` `name` argument and fields; `ask` `approval`, `approval_id`; new `comments_handoff`; new error codes), `crates/clax-mcp/src/render.rs` (`clax-clip:` rewrite), `crates/clax-hooks/src/*.rs` (clip rewrite in Stop and prompt payloads; no `codex queue` assumptions), `plugins/pi/src/{client,remote,clax}.ts` (remote mode with `node:crypto` Ed25519 keys read from `remote.key`, new arguments, `clax_comments_handoff`), `scripts/sync-skill-tools.py` (25 tools), `docs/contract.md` (Tools)
- Test: `crates/clax-mcp/src/tools.rs` tests, `crates/clax-server/tests/api_mcp.rs`, `plugins/pi/test/*.test.ts`

- [ ] **Step 1: Write the failing tests**

`status_sets_and_reports_the_name` (`name_taken` passes through as a tool error), `ask_with_approval_returns_the_decision`, `comments_handoff_moves_and_releases`, `remote_clip_lines_are_rewritten_to_cached_files` (a fake remote serving a PNG; the cache file exists and the text names it; a 403 rewrites to `Clip: unavailable (withheld)`), `tool_count_is_25`; Pi: `remote mode signs with the same canonical string as Rust` (a fixed key and input produce the Rust test vector from Task 3), `clax_comments_handoff is registered`.

- [ ] **Steps 2–4**, **Step 5: Commit** — `"Expose agent names, approval requests and thread handoff to agents in every harness"`

---

### Task 17: The shell: People, approval and connection cards, agent strip, owner chips

Spec §17, §9.5, §13, §3. **Before starting, ask the owner the batch of UI questions in spec Q9 and record the answers in the task's report; do not begin the UI until they are answered.**

**Files:**
- Create: `web/shell/src/people/` (`People.svelte`, `PersonCard.svelte`, `InviteDialog.svelte`, `ApprovalCard.svelte`, `ConnectionCard.svelte`, `AgentStrip.svelte`, `model.ts`, tests beside each)
- Modify: `web/shell/src/api.ts` (people, grants, approvals, agents), `web/shell/src/q/` (inbox renders `approval`, `connection`, `ended`; mounts `AgentStrip`), `web/shell/src/ui/` (top-bar People link; thread card owner chip and `withheld` delivery state; Send menu by name grouped by person), `crates/clax-server/src/routes/shell.rs` (`/people` serves the gallery entry), `web/perf/bundle-budget.json` (`people`: 16384)
- Test: Vitest component tests; `web/e2e/people.spec.ts`

- [ ] **Step 1: Write the failing tests** — `ApprovalCard` renders author, agent, thread, content as text (hostile `<img onerror>` and U+202E stay text), four buttons, and calls `decide` with each scope; `ConnectionCard` requires the typed code before **Approve** enables; `AgentStrip` orders blocked-longest first and formats "waiting on you 12 min" from a fake clock; People shows the `*unconnected` switch for the owner and not for a person; the Send menu lists "Your agents" then "Sam's agents" with "Sam approves first".
- [ ] **Step 2: Run** (`npm --prefix web run test -- people`) — fails. **Step 3: Implement.** **Step 4: Verify in a real browser**: `just dev claude`, a second signed client from `scripts/dev-second-person.sh` (create: connects a temporary home to the dev daemon and registers an agent), load `/people`, `/inbox` and an artifact; screenshots at 1280 px and 390 px, light and dark, of: invite dialog, connection card, approval card, agent strip, owner chip, Send menu. Attach them to the report.
- [ ] **Step 5: Commit** — `"Add People, approval and connection cards, the agent strip and owner chips to the shell"`

---

### Task 18: The extension's panel

Spec §9.5 (extension), §17.

**Files:**
- Modify: `web/extension/src/panel/` (Inbox tab renders `ApprovalCard` from the shell's `people` module; the card above a live page's thread; agent names in the Send menu), `web/extension/src/sw/inbox.ts` (approval routes through the credentialed API)
- Test: extension Vitest tests; `web/e2e/chrome-overlay.spec.ts` addition

- [ ] Steps: failing tests (`panel shows an approval card and decides it`, `send menu names agents`), implement, verify in Chromium with the unpacked extension (screenshots), commit — `"Show approval cards and agent names in the extension's panel"`

---

### Task 19: Skills and contract

Spec §16 (skills), §15.

**Files:**
- Modify: `plugins/*/skills/clax/SKILL.md` ("Working beside other agents and people"), `docs/contract.md` (People and grants; Rights; Visibility and placeholders; Approvals; Agent names; Thread ownership; Remote mode; error codes `not_permitted`, `withheld`, `thread_taken`, `name_taken`, `name_locked`, `invalid_name`, `nothing_withheld`, `recently_declined`, `agent_cannot_read`, signature errors), `docs/superpowers/specs/2026-09-28-clax-design.md` (§1 "Out of scope": multi-user now specified elsewhere; D-table pointer)
- Test: `scripts/sync-skill-tools.py --check`, the contract test in `crates/clax-mcp/src/tools.rs` (`contract.json` matches)

- [ ] Steps: update, run checks, commit — `"Document people, rights, visibility, names and ownership for agents and in the contract"`

---

### Task 20: Single-owner golden, perf gate and two-person end to end

Spec §19, §23, §24.

**Files:**
- Create: `crates/clax-server/tests/golden_single_owner.rs`, `crates/clax-server/tests/fixtures/single-owner-21.sql` (a migration-21 database dump with sessions, artifacts, versions, threads with LAN viewers' comments, feedback in every state, docs, questions, inbox), `web/e2e/visibility.spec.ts`
- Modify: `scripts/perf-daemon.py` (two-person seed; `vis_threads_ms`, `vis_feedback_poll_ms`, `sig_verify_us`; a signed polling client beside the probe loads), `scripts/perf-daemon-budget.json`

- [ ] **Step 1: Write the failing tests**

```rust
/// Agent-facing outputs on a migrated single-owner install equal main's, apart from the names M17 adds.
#[tokio::test]
async fn single_owner_outputs_are_unchanged() {
    let ts = TestServer::from_fixture("single-owner-21.sql").await;
    let golden: Value = serde_json::from_str(include_str!("fixtures/single-owner-21.golden.json")).unwrap(); // produced by main's build: see Step 3
    let got = collect_agent_outputs(&ts).await; // comments_read of every thread, read/list, feedback for every tier, notices, roster, inbox
    assert_eq!(strip_names(got), golden);
}
```

`web/e2e/visibility.spec.ts`: the owner invites Sam (People), a signed test client connects as Sam and registers an agent; Mia (a second client) comments and sends to Sam's agent; Sam's browser (the browser link) sees the approval card, approves the thread; the agent's feedback poll then carries the comment; screenshots of each step at desktop and phone width.

- [ ] **Step 2: Run** — fails. **Step 3:** produce `single-owner-21.golden.json` by running `collect_agent_outputs` on main's build (`git worktree add` of main in the scratch directory, `cargo test -p clax-server --test golden_single_owner -- --ignored --exact write_golden`), commit the file; implement the perf phases. **Step 4: Run** `scripts/quality_gates.sh` (whole suite within about 2 minutes), `scripts/perf-daemon.sh`, `npm --prefix web run e2e -- visibility people`. **Step 5: Commit** — `"Pin single-owner behaviour with a golden, budget the filter, and test two people end to end"`

---

## Audit adoption (no task; for the merge)

When `toolpath-audit` merges, each store function named in spec §20.2 already returns what its event needs (`RevokedGrant`, `Decided`, `TakeResult.placeholders`, `EndedSession`, …); the merge adds the `audit::record` call in the same transaction and the actor fields of §20.1. Whichever branch merges second renumbers its migrations (Global Constraints) and extends the audit backfill to grants, approvals and requests (`backfilled: true`).

## Self-review notes

- Every decision O1–O5 maps to tasks: O1 → 2, 4, 15, 17; O2 → 1, 5, 14; O3 → 7–11, 16–18, 20; O4 → 6, 12, 13, 17; O5 → 1–4 (spec §4).
- At no commit is unfiltered content served to a person's agent: Task 5 refuses every `Filtered` route to grants (`503 not_yet`); Task 8 lifts it for the read routes once they filter, and Task 9 for the feedback poll and notices once every tier filters. Owner-token agents see only the owner's and `*unconnected` content until a person exists, which needs Task 4's routes and no UI.
- Tasks 1–3 and 6–7 are pure or store-level and can be reviewed alone; Tasks 5, 8 and 9 carry the security weight and get the deepest review.
