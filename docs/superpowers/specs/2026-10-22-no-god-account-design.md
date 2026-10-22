# No god account on a hosted Clax

Status: draft for the owner's review (2026-10-22).

## 1. Intent

On a hosted Clax (clax.dev, or any daemon with `[hosted]`), the owner and
any person they grant powers to are users like any other, with extra
powers. Seeing everyone's artifacts is not one of them. No credential on a
hosted instance reads the whole instance through Clax: not the owner's
browser, not `CLAX_TOKEN`, not the owner's agents, not a person the owner
granted rights at `*`.

The owner keeps every power that is about the instance and its people:
setup, invites, People (rights, revocation), sign-in links, agent tokens,
their own sessions, jobs and approvals. The owner may grant any rights or
roles. What a grant can reach is bounded by what the granter can reach.

## 2. Owner decisions

- D1. Hosted only. A local daemon keeps its model (its owner reads
  everything; that owner holds the database file anyway).
- D2. Every owner credential on hosted is scoped: the owner's browser
  session (and its shell token), the `CLAX_TOKEN` bearer, the owner's
  agents (`OwnerAgent`), and grants of the owner's row
  (`PersonAgent { person: None }`).
- D3. Moderation: the owner revokes a person (as today) and sees
  per-person counts (artifacts, storage); never titles or content of
  artifacts they cannot reach.
- D4. ClaxBot (the `claxbot` branch: admin role, runs, audit) is out of
  scope; that branch adapts to this rule when it rebases.
- D5. The owner can grant whatever roles and rights, but no account sees
  the world: there is no god account.
- D6. On hosted an invite only makes a new person; a revoked person is
  reinstated by un-revoking, never by a link the owner holds (§4.3).
- D7. Revoking a person stops serving everything they made, to everyone,
  while revoked; reinstating brings it back (§4.3).

## 3. The reach rule

On a hosted daemon, for any caller acting as person P (the owner, a
connected person, or an agent of either):

- **Granted rights** (the rights stored on P's viewer row, the owner's
  `Rights::full()`, a grant's rights, which are already narrowed to its
  person's) apply only at artifacts whose creator is the owner
  (`artifacts.person IS NULL`) or P (`artifacts.person = P`).
- **Share rights** (`clax_store::shares::share_rights`, an artifact shared
  with P by its maker) apply at their artifact, whoever made it.
- **Link rights** (`LinkAccess::rights`) apply as today.
- At any other artifact P has nothing: routes answer it as one that does
  not exist (404), listings leave it out, streams drop its topics.

A location of `*` therefore means "every artifact the owner made, and
mine" on hosted. `own` keeps its meaning. A location naming another
person's artifact ID in granted rights has no effect.

A live page (`live_ids`) is the owner's browsing; it stays the owner's
alone, as today.

## 4. Where it applies

### 4.1 One scoped caller

`clax_http::hosted::MemberRights` becomes the scope of every person on
hosted, the owner included:

- It holds the person (`None` for the owner: `own` resolves to
  `person IS NULL`), the granted rights, and the share rights, separately,
  so the reach rule can apply to the granted ones alone.
- `MemberRights::allows_at` applies the reach rule, then shares, then the
  link.
- The gate (`clax_http::hosted::gate`) builds it for the owner's session
  (cookie path and shell-token path, where it is now `member: None`) and
  for a daemon-token `Authorization` request, and runs `member_gate` for
  them as for a member.
- `Agent<N>` (`clax_http::agent`) takes the hosted scope for every caller
  that carries one, not only `Caller::Person`, so the per-route right,
  `named_artifact`, `also_needs` and `read_rights` (listings) all apply it.
- `PersonAgent` checks (`allowed_at`) and `ReadRights::allows` apply the
  reach rule on hosted too, so a grant of a person (or of the owner's row)
  reaches no further than that person.
- `session_reads` and `reads_comments` lose their `owner => true` arms on
  hosted.

### 4.2 Paths that skip rights today

Each of these keys on the owner or the daemon token, not on rights, and
must take the scope on hosted (file references are to `main` at
b0150ae3):

1. `GET /api/threads` (`crates/clax-server/src/routes/threads.rs`,
   `TokenOrAgent`): only threads of artifacts in reach.
2. Stream (`crates/clax-server/src/routes/stream.rs`): the gallery topic
   and `artifact:`, `docs:`, `presence`, `working` topics filtered as for
   a member; draft topics need the artifact in reach, not only the token.
   `/api/events` (daemon-wide) is refused to every hosted caller, as it is
   to members now.
3. db (`crates/clax-http/src/db_caller.rs` `base_caller`): the token's
   `Level::Owner`/`Level::Admin` only at artifacts in reach; elsewhere the
   caller's scoped level.
4. Content keys (`crates/clax-http/src/hosted.rs` `content_key`,
   `asset_key`, `clip_key`): on hosted, keyed by a random secret the store
   generates once and keeps in the `install` table, never by the daemon
   token, and never sent anywhere. Holding `CLAX_TOKEN` must not let
   anyone compute a content address.
5. Toolpath export (`crates/clax-server/src/routes/toolpath.rs`
   `owner_reads_history`): events on artifacts out of reach are left out;
   events with no artifact stay.
6. Git remotes (`crates/clax-http/src/git_remotes.rs`, `Asker::Owner`):
   the owner's asker is scoped; fork lineage only through artifacts in
   reach.
7. `GET /api/agents` (`crates/clax-server/src/routes/people.rs`,
   `Handles::All`): an agent's `working` entries name only artifacts in
   the caller's reach; others are counted, not named.
8. Inbox (`crates/clax-http/src/views/inbox.rs`): items of artifacts out
   of reach are not listed.
9. Drafts (`crates/clax-server/src/routes/drafts.rs`) and hidden or draft
   artifacts: in reach only.
10. Administering (`clax_http::agent::administers`) and sharing
    (`clax_server::hosted::may_share`): on hosted, an artifact's maker
    alone (the owner for `person IS NULL`). The owner no longer shares,
    pins, tags or changes the capabilities of a member's artifact.
11. Tags (`crates/clax-server/src/routes/tags.rs`): listings and counts
    only over artifacts in reach.
12. `brief_refusal`'s owner arm (`crates/clax-server/src/routes/artifacts.rs`)
    and every other `Caller::Owner`/`is_owner()`/`holds_token` access
    decision the implementation finds: each either takes the scope or is
    shown not to name an artifact.
13. Worker diagnostics (`crates/clax-worker/src/diag.rs`, behind
    `CLAX_DIAG_ROUTES=1`): repo tokens and repo reads only for artifacts
    in reach.

### 4.3 People

People keeps revocation. A person's card shows counts (artifacts made,
storage used) and never titles or IDs of artifacts out of the owner's
reach.

- **Invites make new people only (D6).** On hosted, an invite naming an
  existing viewer row (`InvitePerson::Viewer` in `Store::create_invite`,
  `crates/clax-store/src/people.rs`: a person, a revoked person or a
  guest) is refused, so no link the owner holds signs a browser in as
  someone else. The same holds for every other path that signs a browser
  or a machine in as an existing person on the owner's behalf (sign-in
  links are made by the person themselves; connecting a machine as a
  person): the builder confirms each.
- **Reinstating (D6).** A new owner route un-revokes a person: it clears
  `person_revoked_at` and the GitHub block revocation set, and hands out
  no credential. The person signs in again themselves (GitHub). Their
  rows, artifacts and shares are as they were.
- **Revocation takes their pages down (D7).** While a person is revoked,
  nothing they made is served to anyone: not by link, not by share, not
  in the public gallery, not at content addresses (`/c/…`, `/_blob/…`,
  `/_clip/…`), not through git. Routes answer 404 as for an artifact that
  does not exist. Nothing is deleted; reinstating brings it all back. The
  owner never sees the titles or IDs of what was taken down, only counts.
  Deleting everything a person made is out of scope.

## 5. Non-goals

- Protection from whoever operates the storage (Durable Object storage,
  the database file, R2). The guarantee is about what Clax hands out.
- The local daemon (D1).
- ClaxBot's admin paths (D4).
- A report-to-moderate flow.

## 6. Anti-goals

- A fallback, flag or "break glass" that restores universal reading.
- A count or listing that leaks the titles, IDs or content of artifacts
  out of reach (a 403 that confirms an artifact exists counts as a leak:
  answer 404).

## 7. Constraints

- `docs/contract.md` changes with the code: the owner rules
  ("Security model", hosted, §8.5 "the owner's browser sees every
  artifact", People, streams, db levels, Toolpath export).
- Existing members' behaviour is unchanged except where their stored
  rights named `*` or another person's artifact (now bounded by the
  reach rule).
- Tests: HTTP rights tests for the owner's session, `CLAX_TOKEN`, an
  owner's agent, an owner-row grant, a member granted `*` and a member,
  against an artifact of another member, one shared with them, one
  shared by link, and the owner's own, on each path in §4.2; and a
  revoked person's link-shared, shared and public artifacts, before and
  after reinstating (§4.3).
- No migration: the content-key secret lives in the existing `install`
  table.

## 8. Settled while writing (the author's calls, not the owner's)

- Sharing with the owner: a maker may share an artifact with the owner as
  with anyone (`already_owner` in `crates/clax-store/src/shares.rs` is
  dropped on hosted), since the owner no longer reads it otherwise.
- Elevations (`crates/clax-store/src/elevations.rs`): an owner's
  elevation may name only locations in the owner's reach.
- Cross-artifact reads (`db_read_grants`, `/api/agent-read-consents`):
  a source artifact counts only if it is in the caller's reach, and a
  source out of reach answers as one that does not exist.
- Jobs and threads sent to the owner's agents that name artifacts out of
  reach fall under §4.2 item 12.

## 9. Left to the builder

- How `MemberRights` represents the owner (an `Option` person, an enum).
- Whether `OwnerAgent`/`Owner` callers on hosted are rewritten into a
  scoped variant or carry the scope beside them.
- The order of the §4.2 items and how to find every remaining owner
  bypass (a grep for `is_owner()`, `Caller::Owner`, `OwnerAgent`,
  `holds_token`, `who.token`, `TokenOrAgent`, `RequireToken` finds about
  320 sites in about 40 files; most are owner powers, not artifact
  access).
- Content-key rotation for existing hosted instances: old `/c/…` URLs
  stop working once keys move to the new secret; that is acceptable.
