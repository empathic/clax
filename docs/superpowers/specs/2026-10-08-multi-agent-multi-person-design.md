# Multi-agent and multi-person Clax

Date: 2026-10-08
Status: revised after review (revision 5); the owner's answers to the first
draft's open questions are recorded as decisions O6–O9 and R1–R4
Depends on: `2026-09-28-clax-design.md` (the main spec: participants and
attention §10, sessions and the owner identity §11, the MCP surface §12,
security §14), `2026-10-05-chrome-overlay-design.md` (the extension
gateway L5, the owner identity L6, live pages L10) and
`2026-10-06-agent-questions-and-inbox-design.md` (questions, the inbox).
Interacts with: `2026-10-06-toolpath-audit-design.md` (on main since
migration 23; §20).
Model: RFC 9635, the Grant Negotiation and Authorization Protocol (GNAP),
https://www.rfc-editor.org/rfc/rfc9635.txt (§4 maps it and states every
departure).

## 1. Purpose

Today one person owns a Clax install and every agent that holds the daemon
token is theirs. Two things change that:

- **Several agents of one person** attach to one Clax (two Claude Code
  sessions in two worktrees, a Codex session beside them). The person needs
  to tell them apart, send a thread to exactly one, see which one took it,
  and see at a glance which one is waiting on them.
- **A second person's agents** attach to the same Clax from another machine
  on the LAN. Comments, replies, pages and documents written by one person
  then become input to another person's agents, which is a prompt-injection
  path, and a person's agents must not be handed content their person has
  not agreed to.

Owner's statements (2026-10-08, verbatim, in order):

- "How should we make it so that multiple agents attached to the same clax
  have a good time and humans routing to them are happy as well?"
- "How would we deal with prompt injection risk and routing approval if two
  people have their own agents connected to the same clax?"
- "Even more than making origin visible, we may want to not show some
  message A to some bot B if B's "owner" doesn't approve of A reaching them"
- "Please and implement it"

### Goals

- The owner connects a second person's agents by invite or by approving an
  ad-hoc request; each machine then holds a key-bound credential tied to
  that person, revocable per machine and per person.
- An agent has its person's rights. The person narrows them; the owner may
  give one agent more.
- Content written by someone other than an agent's own person is never
  handed to that agent by any Clax agent channel (tools, hooks, feedback
  tiers, notices, the signed client), field by field, until the agent's
  person approves it (per author per thread, or "always allow" an author on
  an artifact or everywhere). The agent sees a content-free placeholder and
  can ask its person for approval.
- Agents have meaningful names; `@name` reaches exactly one agent.
- A sent thread has one owner agent; others see it taken; handoff is
  explicit; ownership is released when the session ends.
- Ended sessions clean up after themselves, with one inbox note.
- The inbox and the roster are grouped by agent and say which agent has been
  blocked on the person, and for how long.
- A single-owner install upgrades with the changes listed in §19 and no
  others.

### Non-goals

- Accounts, passwords, single sign-on, or anything that leaves the LAN.
  Clax stays local; a "person" is a viewer identity of this install.
- TLS this round (O7). The daemon stays HTTP; §21 says what that costs and
  what the signatures still protect.
- An access-control boundary on a public LAN. The gallery, threads and
  pages stay public to every LAN viewer, as today; the filter governs what
  Clax's agent channels hand an agent, not what an agent can fetch as an
  anonymous LAN viewer with a web or browser tool (§21).
- Agent-to-agent messaging, and agents approving content for other agents.
- Hiding content from people. The filter is agent-facing only.
- Live pages for other people. Live pages stay visible only on the owner's
  machine (chrome-overlay L10); other people's agents never see them.
- Removing content an agent has already received. Revocation stops future
  delivery; nothing can be unread.

## 2. Decisions

O1–O5 are the owner's decisions of 2026-10-08; O6–O9 are the owner's
answers to the first draft's open questions, and R1–R4 the coordinator's
rulings where the owner deferred to the draft's recommendations, the same
day. All are binding. M1–M27 are this design's decisions, each with its
reason.

| # | Decision | Reason |
|---|---|---|
| O1 | Connecting a second person's agents: both a one-time invite from People (`clax connect <invite>` on the other machine) and an ad-hoc request (`clax connect http://host:port`, which the owner approves in the inbox). Credentials are tied to the person's identity and revocable per person. | Owner decision. |
| O2 | An agent has its person's rights by default. Its person can narrow the rights of their own agents. The owner can grant one of another person's agents more than that person has. | Owner decision ("not propose only"). |
| O3 | Content from other people reaching an agent is withheld until its person approves: per author per thread, or "always allow <person>" scoped to an artifact or everywhere. The daemon enforces it on every agent-facing read and delivery path: comments, replies, anchor quotes from pages others authored, clips, db documents written by others, artifact files and versions authored by others' agents, all feedback tiers including push and `wait_for_feedback`. Agents see a content-free placeholder and can use `ask` to request approval. Approvals batch per author per thread; revocation stops future delivery. Solo setups are unchanged; today's owner-token agents count as the owner's. | Owner decision. |
| O4 | Also in scope: meaningful agent names (derived from repo, branch and task, agent-settable, person-renameable; `@name` routes to exactly one agent); thread ownership and handoff; agent lifecycle cleanup (ended sessions leave pickers, their questions withdrawn, threads released, one inbox note); agent-aware attention. | Owner decision. |
| O5 | Model permissions and capabilities on GNAP (RFC 9635): grants, access rights, interaction, continuation and token management, and key binding where it fits a local daemon; adapt pragmatically and say where and why the design departs. | Owner decision. |
| O6 | LAN viewers who are not connected as people: their comments reach the owner's agents without approval (today's behaviour on every install), with a plain switch in People, shown again beside the first invite. | Owner answer (first draft's Q1). |
| O7 | No TLS this round; document "use a trusted LAN"; a later design may add a self-signed certificate whose fingerprint the invite carries. | Owner answer (Q3). |
| O8 | An untargeted send with several of the owner's agents watching: the first to take it owns it. | Owner answer (Q5). |
| O9 | Other people's artifact titles in an agent's `list` are withheld until approved. | Owner answer (Q8). |
| R1 | A thread approval covers the author's later replies in that thread; the approval card says so. | Ruling (Q2). |
| R2 | Only the agent's own person approves content for it; the owner cannot approve on another person's behalf. | Ruling (Q4). |
| R3 | Cross-person agent name collisions get a suffix; the person is shown beside the name. | Ruling (Q6). |
| R4 | Other people's extensions pair with their own daemon: out of scope. | Ruling (Q7). |
| M1 | **A person is a viewer.** A connected person is a `viewers` row with `person = 1`; the owner is the owner row. No parallel "principal" table. | Viewers already carry the public ID, the name, authorship, mentions, attention and claiming. A person's browser and their agents share one identity, as the owner's do. |
| M2 | **An agent is a session.** Each agent is a `sessions` row bound to its person (`sessions.person`, NULL for the owner) and, for another machine, to the grant it registered under (`sessions.grant_id`). Rights narrowings and elevations for one agent live on its session and end with it. | Sessions already identify agents. Agents are short-lived; per-agent state that outlives them would need its own lifecycle. |
| M3 | **One grant per person per machine.** `clax connect` makes one GNAP grant for the person's Clax install on that machine (GNAP's client instance). Every agent session on that machine registers under it. | Agents are many and ephemeral; one key per machine is what a person can manage and revoke. |
| M4 | **Sessions are bound to their credential.** Registration, refresh and join match only rows with the caller's `grant_id` (`grant_id IS NULL` for the daemon token); a grant never matches, lists, reads or modifies another grant's or the token's session; every session named in a path or in `X-Clax-Session` must be the caller's (403 `foreign_session`). | PIDs collide across machines; the existing match by `(harness, parent_pid)` would otherwise hand one person's agent another's session, its feedback and its threads. |
| M5 | **Tokens are key-bound, never bearer, and live with their grant.** `clax connect` makes an Ed25519 key pair; the private key never leaves the machine. Every request carries the access token and a signature over the method, path, query, host, a timestamp, a nonce, the body digest and the token (§6.6). Access tokens do not expire and are not rotated; revoking the token (through the manage URI) revokes the whole grant, as revoking the grant does, and the person reconnects. Every signature is verified before the handler runs, on every request, with or without a body. The owner's daemon token (local, file mode 0600) is unchanged. | The LAN is plain HTTP: a sniffed bearer token would be reusable. A signature makes a sniffed token useless alone and a sniffed request useless twice. Expiry and rotation add little to a key-bound token, and can strand a sleeping machine. |
| M6 | **Rights are GNAP access objects** with Clax types (`clax-artifacts`, `clax-comments`, `clax-db`, `clax-ask`), actions, and locations (`*`, `own`, or artifact IDs), plus reference presets (`clax:reader`, `clax:collaborator`, `clax:full`) (§7). An agent's effective rights are `(person ∩ grant narrowing ∩ agent narrowing) ∪ owner elevation`. | O2 in one formula. |
| M7 | **Every route declares what an agent needs on the route itself.** A typed extractor (`Agent<N: Need>`) takes the artifact from the same `Path` the handler uses and checks the right; the route table of §7.5 is the contract, and the guard test reads it off handler signatures. | One router: no second path matcher beside axum's to drift from it. |
| M8 | **One `Caller`, and every agent channel is an agent.** Every handler learns who it serves from one `Caller` (§6.1); no handler reads `Authorization` or checks the token itself. A token request is an owner agent when it names a session (path or `X-Clax-Session`) or carries the agent-channel marker `X-Clax-Channel: agent`, which every agent-side client sends (the shim, `/mcp`'s internal client, the hooks, `clax feedback follow`, Pi), and the CLI whenever it runs under a harness: through the plugins' wrapper (which exports `CLAX_CHANNEL=agent`: slash commands, hooks), or in a harness's shell (an agent's shell tool, Claude Code's `!` commands), detected from the variables each harness sets (§16); only the CLI run outside any harness, or with `CLAX_CHANNEL=owner`, is the owner. | About twenty handlers decide agent-ness with an inline token check today; with a grant they would misfile an agent's reply as an anonymous comment. |
| M9 | **Visibility is separate from rights.** Rights say what an agent may do; visibility (approvals) says whose words may reach it. Approvals are given by the agent's person (R2). | O3: B's person decides what reaches B. |
| M10 | **One filter, field by field.** `Visibility` (in `clax-core`) answers, for one field, "may an agent of reader R see this field, whose effective authors are A₁…Aₙ, at artifact Y (thread T)?" Every field of every item an agent channel returns that has authors of its own (a comment's body, a thread's anchor, a clip, an artifact's title, a version's note and files, a document, a working message…) is checked on its own and replaced by its placeholder when withheld, in reads, write responses and every delivery tier alike. | A visible comment on a page someone else wrote still carries that page's quote and title; filtering whole rows would leak them. |
| M11 | **Effective authors** (§8.2): content carries the set of people whose words it holds. A comment, document or upload a page wrote, a page's self-published version and a page's metadata change also count every author of the page: the authors of every file of its current version, fixed at write time (a script carried forward from another person runs in the page as much as its `index.html`). A version's file carried forward keeps the authors it had; an artifact's text field keeps the authors of what last set it; a document's author set only grows until the document is deleted. | Page-written content is the confused-deputy path: a page running in the owner's browser writes "as the owner"; carrying a file forward or rewriting a document must not move its words to a new author. |
| M12 | **An agent's own person's content is always visible.** | The filter guards against other people's words, not one's own. |
| M13 | **Approvals are per author, at three scopes**: one thread (including later replies, R1), one artifact, or everywhere. An author is a person; their agents' words count as theirs. | O3's wording. |
| M14 | **`*unconnected`.** Migration 25 gives the owner one approval for the author class `*unconnected` (every viewer who is not the owner and not a connected person, and anonymous callers) everywhere (O6). Connected people are never covered by it as themselves; their unsigned browsers and scripts are, which People states beside the switch (§9.6). | Today's reach for LAN viewers on every install, with its consequence shown. |
| M15 | **Connecting asks the connecting person about the owner.** `clax connect` asks "Let <owner>'s comments and pages reach your agents everywhere? [Y/n]" and records the answer as that person's approval. | A person who connects to someone's Clax is there to work on that person's pages. |
| M16 | **Withheld bodies stay undelivered.** A feedback row whose comment body is withheld is not handed over; its session gets one placeholder per (author, thread) (`placeholder_at`), and the row is delivered once approved. A row whose body is visible is delivered with every other withheld field placeholdered. | Delivery semantics (once, acknowledged, resent) stay as they are for visible bodies. |
| M17 | **Visibility and rights are current at every take.** Each take (and each wake of a long-poll) reads a generation counter and rebuilds `Visibility` and the rights when it changed. Every approval, approval revocation, rights or narrowing or elevation change, grant or person revocation, and viewer claim or fold (which moves authorship) bumps it and wakes the affected sessions' waiters. A waiter registers before it takes, so a bump between its take and its wait is never missed. | A long-poll may wait ten minutes; a revocation must apply to its next take. |
| M18 | **Routing is an approval request.** A send that names an agent (the picker, `to`, `@name`, handoff, a batch naming one, or a page's `sendToClaude` naming one) whose person has not approved the thread's authors makes an approval request to that person at once. A send naming no agent asks no one: other people's watching agents get the placeholder and may `ask`. | O3's "routing approval"; owner decision 2026-10-11, "Only named sends ask". |
| M19 | **Approval requests batch per (approver, author, artifact, thread).** | O3's batching. |
| M20 | **Approving, rights, elevation, narrowing and connecting need a browser.** Deciding a request, approving directly, revoking an approval, setting rights, elevating, narrowing, making or revoking invites, and approving, denying or revoking grants take a browser credential (the owner cookie, the extension, or a connected person's cookie) on a same-origin request (`SameOrigin`); the daemon token alone is refused (403 `browser_required`). The CLI prints the People link for these. | Approvals then cannot be made through the Clax tools, the CLI, or by any remote agent, which holds no token and no browser cookie. An owner agent with shell access is not stopped: it runs as the owner's user and can fetch the owner's browser credential (`/api/token`), so it is trusted at the owner's level (§21.3); no mechanism can hold against a process of the same user. |
| M21 | **The inbox gains a recipient.** `inbox_items.recipient` is the person it is for (NULL for the owner). | Questions, approvals and attention are per person; the owner's inbox stays as it is. |
| M22 | **Agent names are live-unique tokens.** `[a-z0-9][a-z0-9._-]{0,31}`, unique (ignoring case) among live sessions, never a reserved word or a person's name in token form; derived from repo and branch, set by the agent (`status {name}`), renamed by its person or the owner (§11). | `@name` must reach exactly one agent and never be confused with a person's mention. |
| M23 | **The thread's owner agent is its target session.** `threads.target_session_id` is the owner: set by a send with `to`, `@name`, handoff, or by an implicit take: the first session to take a delivery of an untargeted send, or the first reply, by an agent of the person who sent the comment taken (O8 decides first take among one person's agents), which deletes that person's other agents' undelivered rows for it. Another person's agent becomes the owner only by a send naming it or a handoff. Released on the owning session's end, its grant's revocation, or by the agent. | The existing column already means "where this thread goes". |
| M24 | **Handoff is a tool** (`comments_handoff`); taken threads refuse other agents' replies and resolves (409 `thread_taken`). | Explicit, and two agents never answer one thread. |
| M25 | **Session end cleans up** and makes one `ended` inbox note when the session owned threads or had open questions or approval requests. This reads O4's "one inbox note" as one note per ended session that had something to clean up; a session that ends with nothing open makes none. | O4, interpreted so idle sessions make no noise. |
| M26 | **Another machine is a remote, not a second daemon.** `clax connect` writes `<home>/remote.json` and the key; while it exists, the shim, hooks, CLI and Pi extension talk to that daemon (signed) instead of starting one. | One switch in the client keeps every harness working unchanged. |
| M27 | **Migrations 24–26** (people and grants; visibility; per-person inbox), numbered after main's 23 (the audit journal). The numbers are relative: each schema task takes the next free number on main when it is implemented, and amends this text. The `toolpath-audit` branch landed on main first and kept 23; these follow it. | Contiguous migrations; no build of an unmerged branch has run on a real home. |

## 3. User flows

### 3.1 Inviting Sam

1. The owner opens **People** (`/people`) in a browser, chooses **Invite a
   person**, types "Sam", leaves the preset at **Collaborator** (§7.3),
   and presses **Create invite**. People shows
   `clax connect http://192.168.1.5:7480/i/K7QF-M2XD-9PLA`, "expires in
   24 h, one use", "Use a trusted LAN: Clax traffic is not encrypted", and,
   beside the first invite, the `*unconnected` switch with its text
   (§9.6). With the daemon bound to loopback only, People says to restart
   it with `clax serve --bind 0.0.0.0` first; no invite is made.
2. On Sam's machine, Sam runs that command. It makes a key pair, redeems
   the invite, prints the key's fingerprint and "Connected to Alex's Clax
   as Sam", asks "Let Alex's comments and pages reach your agents
   everywhere? [Y/n]", writes `~/.clax/remote.json`, and opens Sam's
   browser at a one-time link that makes that browser Sam (§6.5).
3. The owner's inbox gets "Sam connected from sam-mbp (key SHA256:9f…)".
4. Sam's agents (any harness with the Clax plugin) now reach the owner's
   daemon, each registering as Sam's agent with a derived name.

The owner connects a second machine of their own the same way, with an
invite for themselves (**Invite → Me**).

### 3.2 An ad-hoc request

1. Sam runs `clax connect http://192.168.1.5:7480`. The command prints
   "Ask the owner to approve request **WXYZ-4821**" and waits (polling the
   grant's continuation with GNAP's `wait`), up to ten minutes.
2. The owner's inbox shows "Connection request from sam-mbp
   (192.168.1.40), key SHA256:…". **Approve…** asks for the code shown on
   the requesting machine (a mismatch approves nothing), a name for the
   **new** person, and the preset. Adding a machine to an existing person,
   or to the owner, takes an invite (§6.3); the card says so and links to
   **Invite**.
3. Sam's command receives its token and continues as in 3.1 step 2.

### 3.3 Withheld content and approval

1. Sam's agent `clax-multi-person` watches the owner's artifact "Quarterly
   Review". Mia (another connected person) comments on it and presses
   **Send to clax-multi-person**.
2. Sam has not approved Mia. The daemon stores the thread and the feedback
   rows for Sam's agent, holds them, and makes an approval request in Sam's
   inbox: "Mia wants to reach your agent clax-multi-person · 1 comment on
   Quarterly Review «Quarterly goals»", showing Mia's comment, with **Allow
   this thread (and later replies)**, **Always allow Mia on this page**,
   **Always allow Mia everywhere** and **Decline**.
3. The agent's next tool result carries, instead of the comment:
   `[clax] 1 comment from Mia on "Quarterly Review" (thread 01J9…) is
   withheld until your person approves it. Your person has been asked.`
4. Sam presses **Allow this thread**. The held rows become deliverable and
   reach the agent by the usual tiers.
5. Without routing (Mia comments on a thread the agent already owns), the
   placeholder says `To ask your person, call ask with {"approval":
   {"thread_id": "01J9…"}}`; the agent's `ask` makes the same request and
   waits for the decision.
6. The owner (approved by Sam at connect) comments on a page Mia's agent
   published and sends it to Sam's agent. The comment is delivered; the
   anchor quote, the clip and the page's title in the same payload are
   placeholders, because they are Mia's (M10).

### 3.4 Narrowing and elevating

- Sam opens People in his browser (his own view: his machines, his agents,
  his approvals) and turns off **Delete artifacts** for all agents of one
  machine, or for one agent. Effective at the agent's next request or take.
- The owner, in People → Sam → agents, gives `clax-multi-person` **Publish
  versions of "Quarterly Review"**. That agent may publish there until its
  session ends; Sam's other agents may not.

### 3.5 Several agents of one person

- The owner has `clax-claude` (Claude Code in `clax` on `main`; `clax`
  is reserved, so the harness is added) and
  `clax-multi-person` (Codex in a worktree). The Send button names one of
  them; its menu lists both by name with their state. A comment
  `@clax-multi-person can you take this?` sends the thread to that agent.
- `clax-claude` takes a thread; `clax-multi-person`'s `comments_read` shows
  it as `taken by clax-claude`; its reply is refused with `thread_taken`.
  `clax-claude` calls `comments_handoff` with `to: "clax-multi-person"`.
- The inbox's agent strip reads "clax-claude · waiting on you 12 min (1
  question)", "clax-multi-person · working on Quarterly Review".
- `clax-claude`'s session ends: its open question is withdrawn, its two
  threads are released, it leaves the Send menu, and the inbox gets one
  note: "clax-claude ended · released 2 threads · withdrew 1 question".

## 4. The GNAP model

### 4.1 Roles

| GNAP role (RFC 9635 §1.1) | In Clax |
|---|---|
| Authorization server (AS) | The daemon. |
| Resource server (RS) | The daemon (same process; departure 1). |
| Resource owner (RO) | The install's owner: approves grants, sets people's rights, elevates agents. |
| End user | The connecting person (Sam), who runs `clax connect`. |
| Client instance | One person's Clax install on one machine: its key pair and `remote.json` (M3). |
| Subject | The person the grant is bound to (a viewer row). |
| Grant | A `grants` row. |
| Access token | A `grant_tokens` row (hash only), key-bound. |
| Continuation | The grant's continuation token and URI: polling and deletion. |

### 4.2 Lifecycle mapped

| GNAP | RFC 9635 | Clax |
|---|---|---|
| Grant request | §2 | `POST /api/grants` with `access_token.access` (rights or a preset reference), `client.key` (`{proof: "clax-ed25519", jwk: {kty: "OKP", crv: "Ed25519", x}}`), `client.display.name`, `interact.start: ["user_code"]`, and for an invite a `clax.invite` code (departure 3). Signed with the new key. |
| Interaction | §2.5.1.3, §3.3.3 | Ad-hoc: the response's `interact.user_code`, which the RO types when approving (departure 9). |
| Asynchronous authorization | §4.4 | The owner approves later, in the inbox; the client polls continuation. |
| Grant response | §3 | `continue {access_token, uri, wait}`, then `access_token {value, manage: {uri, access_token}, access}` (no `expires_in`; the manage token only revokes), `subject {sub_ids: [{format: "opaque", id: <public ID>}]}`, plus `clax {person: {public_id, display_name}, browser_link}`. |
| Continuation: poll | §5.2 | `POST <continue uri>` with `Authorization: GNAP <continuation token>`, signed; `wait` 5 s; 400 `too_fast` sooner. |
| Continuation: modify | §5.3 | Not offered (departure 10). |
| Continuation: revoke | §5.4 | `DELETE <continue uri>`: the person disconnects the machine (`clax connect --off`). |
| Token rotation | §6.1 | Not offered (departure 11): revoke and reconnect. |
| Token revocation | §6.2 | `DELETE <manage uri>` with the manage token. |
| Key proofing | §7.3 | `clax-ed25519` (§6.6; departure 4). |
| RO policy, subject | §3.4, §4 | The person's rights (§7) and `subject`. |

### 4.3 Departures, and why

1. **AS and RS are one process.** No token introspection (RFC 9767); access
   objects are read straight from the store.
2. **One grant per machine, agents as sub-identities (M3).** A grant per
   agent session would put an approval in the owner's inbox for every new
   terminal. Agents are named by `X-Clax-Session` inside a grant, bound to
   it (M4).
3. **Invites are pre-approved grants**, carried as a top-level `clax.invite`
   field of the grant request in place of interaction. GNAP has no invite.
   Redeeming binds the key presented first; a second redemption is 400
   `invalid_invite`.
4. **A narrow key proof instead of HTTP Message Signatures.** `clax-ed25519`
   (§6.6) signs a fixed canonical string with one algorithm, instead of
   RFC 9421's component selection, keeping what §7.3 asks: possession,
   coverage of method, target and body, and freshness.
5. **No finish method, no redirect.** The client is a command line with no
   callback URI; it polls (`wait`). The browser link (§6.5) is not a GNAP
   interaction but the person's browser joining their identity after the
   grant is approved.
6. **No bearer tokens for agents.** `flags: ["bearer"]` is refused. The
   daemon token stays a bearer, local only, as today.
7. **Visibility is not GNAP.** Approvals of content (§9) are the end user's
   consent to what reaches their agents, applied by the RS; they live
   beside grants (M9).
8. **One non-GNAP field**, `level` on `clax-db`, which RFC 9635 §8 allows
   as a type-specific field.
9. **The user code is the RO's check, not the end user's.** RFC 9635
   §3.3.3 has the end user type the code at the AS; here the RO types the
   code the end user reads out, binding the approval to the request on the
   end user's screen. As the RFC says, it is not an authentication factor:
   it proves the owner looked at that request, not who made it, so a
   request can only make a new person (§6.3).
10. **No grant modification.** Narrowing is done by the person from their
    browser (§7.4), not by the client through continuation: one path, not
    two.
11. **Access tokens have no `expires_in` and no rotation.** Tokens are
    key-bound and live while the grant lives; a grant expires after 30
    days unused. The manage URI only revokes (§6.2); a person who wants a
    fresh token revokes and reconnects.
12. **`subject` is returned unasked.** The client needs the person's public
    ID and name to show them; RFC 9635 §3.4 returns subject information only
    when requested.
13. **The `Clax-Grant` authorization scheme** carries the access token on
    resource requests, where RFC 9635 §7.2 uses `GNAP`, so the gateway can
    tell an access token from the continuation and manage tokens, which
    use `GNAP`.

## 5. Data model

All three migrations follow main's 23. Numbers are provisional per M27.

### 5.1 Migration 24: people, grants, sessions, authors

```sql
-- A connected person: a viewer with (or that had) a grant. The owner row
-- is never a person row; the owner's own machines connect as the owner.
ALTER TABLE viewers ADD COLUMN person INTEGER NOT NULL DEFAULT 0;
ALTER TABLE viewers ADD COLUMN person_revoked_at TEXT;   -- a revoked person (§6.4)
-- The person's rights (an access array, §7); NULL: the clax:collaborator preset.
ALTER TABLE viewers ADD COLUMN rights_json TEXT;

CREATE TABLE invites (
    id TEXT PRIMARY KEY,                         -- ULID
    code_sha256 TEXT NOT NULL UNIQUE,
    viewer_id TEXT NOT NULL REFERENCES viewers(id),  -- a person row, or the owner row
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used_at TEXT,
    grant_id TEXT,
    revoked_at TEXT
);

CREATE TABLE grants (
    id TEXT PRIMARY KEY,                         -- 'g_' + 22 base32 chars, public
    viewer_id TEXT REFERENCES viewers(id),       -- the person (the owner row for the owner's machines); NULL while pending
    status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'denied', 'revoked', 'expired')),
    via TEXT NOT NULL CHECK (via IN ('invite', 'request')),
    client_key TEXT NOT NULL,                    -- Ed25519 public key, base64url (32 bytes)
    client_name TEXT NOT NULL,                   -- what the client called itself (untrusted, ≤ 64 chars)
    peer_ip TEXT NOT NULL,                       -- of the grant request
    user_code_sha256 TEXT,                       -- request only, while pending
    wrong_codes INTEGER NOT NULL DEFAULT 0,
    continue_sha256 TEXT NOT NULL,
    manage_sha256 TEXT,                          -- once approved
    narrow_json TEXT,                            -- the person's narrowing of every agent of this grant
    created_at TEXT NOT NULL,
    decided_at TEXT,
    last_used_at TEXT,
    ended_at TEXT,
    ended_by TEXT,                               -- 'owner' | 'person' | 'expiry'
    browser_sha256 TEXT,                         -- the browser link's code (§6.5), until redeemed
    browser_expires_at TEXT                      -- 10 minutes after it was issued
);
CREATE UNIQUE INDEX grants_browser ON grants(browser_sha256) WHERE browser_sha256 IS NOT NULL;
CREATE UNIQUE INDEX grants_live_key ON grants(client_key) WHERE status IN ('pending', 'approved');
CREATE INDEX grants_by_viewer ON grants(viewer_id, status);
CREATE INDEX grants_pending ON grants(created_at) WHERE status = 'pending';

CREATE TABLE grant_tokens (
    sha256 TEXT PRIMARY KEY,
    grant_id TEXT NOT NULL REFERENCES grants(id),
    created_at TEXT NOT NULL,
    revoked_at TEXT
);
CREATE INDEX grant_tokens_by_grant ON grant_tokens(grant_id);

ALTER TABLE sessions ADD COLUMN grant_id TEXT;         -- NULL: registered with the daemon token
ALTER TABLE sessions ADD COLUMN person TEXT;           -- the person's public ID; NULL: the owner
ALTER TABLE sessions ADD COLUMN name TEXT;
ALTER TABLE sessions ADD COLUMN name_source TEXT CHECK (name_source IN ('derived', 'agent', 'person'));
ALTER TABLE sessions ADD COLUMN narrow_json TEXT;      -- the person's narrowing of this agent
ALTER TABLE sessions ADD COLUMN elevate_json TEXT;     -- the owner's elevation of this agent
ALTER TABLE sessions ADD COLUMN ended_reason TEXT
    CHECK (ended_reason IN ('idle', 'exit', 'revoked', 'person'));   -- why it ended (§14)
ALTER TABLE sessions ADD COLUMN ended_watches_json TEXT;   -- idle ends: the direct watches it held, for §6.7's restore
CREATE UNIQUE INDEX sessions_live_name ON sessions(name COLLATE NOCASE)
    WHERE ended_at IS NULL AND name IS NOT NULL;
CREATE INDEX sessions_by_grant ON sessions(grant_id, harness, parent_pid) WHERE grant_id IS NOT NULL;
-- A harness session ID is unique within its credential, never across them (M4):
-- another grant's (or the token's) row with the same ID neither blocks nor matches.
DROP INDEX sessions_harness_id;
CREATE UNIQUE INDEX sessions_harness_id ON sessions(harness, harness_session_id, COALESCE(grant_id, ''))
    WHERE harness_session_id IS NOT NULL AND ended_at IS NULL;

ALTER TABLE artifacts ADD COLUMN person TEXT;          -- the creator's person; NULL: the owner
ALTER TABLE versions ADD COLUMN person TEXT;           -- the publisher's person; NULL: the owner
ALTER TABLE versions ADD COLUMN by_page INTEGER NOT NULL DEFAULT 0;
-- Each file's authors, fixed at publish: a written file's are the publisher's
-- (for a by_page version, with every author of the previous version's files:
-- the page that published it); a carried file keeps the previous version's
-- authors. {"index.html": ["*owner"], "app.css": ["u_…"]}; NULL: every file
-- the publisher's (the owner's, for every version from before this migration).
ALTER TABLE versions ADD COLUMN file_authors_json TEXT;
ALTER TABLE assets ADD COLUMN person TEXT;             -- the uploader's person; NULL: the owner
```

Existing rows get NULLs: every existing session, artifact, version and
asset is the owner's (§19). Existing sessions get no name; names are given
at registration. `person` columns hold public IDs; the viewer claim and
fold (main spec §11) rewrites them with the other author columns.

### 5.2 Migration 25: visibility

```sql
CREATE TABLE approvals (
    id TEXT PRIMARY KEY,                         -- ULID
    approver TEXT,                               -- the person whose agents see; NULL: the owner
    author TEXT NOT NULL,                        -- a public ID, or '*unconnected'
    scope TEXT NOT NULL CHECK (scope IN ('thread', 'artifact', 'everywhere')),
    artifact_id TEXT,
    thread_id TEXT,
    created_at TEXT NOT NULL,
    created_via TEXT NOT NULL CHECK (created_via IN ('shell', 'extension', 'connect', 'migration')),
    request_id TEXT,
    revoked_at TEXT
);
CREATE UNIQUE INDEX approvals_live ON approvals(
    COALESCE(approver, ''), author, scope, COALESCE(artifact_id, ''), COALESCE(thread_id, ''))
    WHERE revoked_at IS NULL;
CREATE INDEX approvals_by_approver ON approvals(COALESCE(approver, ''), author) WHERE revoked_at IS NULL;

CREATE TABLE approval_requests (
    id TEXT PRIMARY KEY,                         -- ULID
    approver TEXT,                               -- NULL: the owner
    author TEXT NOT NULL,                        -- a public ID
    artifact_id TEXT NOT NULL,
    thread_id TEXT,                              -- NULL: content outside threads (a version, a document)
    session_id TEXT,                             -- the agent it is about
    cause TEXT NOT NULL CHECK (cause IN ('ask', 'routing')),
    count INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'approved', 'declined', 'withdrawn')),
    decided_scope TEXT CHECK (decided_scope IN ('thread', 'artifact', 'everywhere')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    decided_at TEXT
);
CREATE UNIQUE INDEX approval_requests_open ON approval_requests(
    COALESCE(approver, ''), author, artifact_id, COALESCE(thread_id, ''))
    WHERE status = 'open';
CREATE INDEX approval_requests_by_session ON approval_requests(session_id, status);

-- A document's authors: every person whose fields it holds (§8.2).
CREATE TABLE doc_authors (
    artifact_id TEXT NOT NULL,
    path TEXT NOT NULL,
    author TEXT NOT NULL,                        -- a public ID, '*owner', or '*unconnected'
    PRIMARY KEY (artifact_id, path, author)
);
-- What a page's write carries (§8.2, M11): every author of the page (of
-- every file of its current version) at the write, for a via_page comment
-- or a page's upload. Same shape as doc_authors.
CREATE TABLE page_authors (
    kind TEXT NOT NULL CHECK (kind IN ('comment', 'asset')),
    id TEXT NOT NULL,                            -- the comment's or asset's ID
    author TEXT NOT NULL,                        -- a public ID or '*owner'
    PRIMARY KEY (kind, id, author)
);
ALTER TABLE feedback ADD COLUMN placeholder_at TEXT;
-- The authors of each of title, description, icon and capabilities, set by
-- the publish or metadata change that last set it (a page's: with every
-- author of the page): {"title": ["u_…"], …}; NULL (and a field it does
-- not list): the creator's.
ALTER TABLE artifacts ADD COLUMN text_authors_json TEXT;
-- The batch's sender as an author: '*owner', a public ID, or '*unconnected'
-- (an anonymous browser). sent_by stays the display name.
ALTER TABLE send_batches ADD COLUMN sender TEXT;
-- Whose agent resolved the thread, for resolved_by 'agent:<harness>':
-- '*owner' or a person's public ID; NULL for a viewer's resolve. Kept with
-- the first resolve, cleared on reopen.
ALTER TABLE threads ADD COLUMN resolved_by_person TEXT;
```

The migration's Rust step then makes the owner's approval of
`*unconnected` everywhere (`created_via` `migration`, a ULID), and the store
records its `approval.grant` (as `system:daemon`) once, as it opens, after
the audit backfill.

Existing documents get no `doc_authors` rows: a document without rows is
the owner's (zero change). Existing `via_page` comments get no
`page_authors` rows, read as the owner (every version before this
migration is the owner's), and existing artifacts no text authors (their
creator's: the owner). Existing batches get no sender, read as
`*unconnected` (a LAN viewer may have sent one), and existing agents'
resolves no person, read as `*unconnected` too.

### 5.3 Migration 26: the per-person inbox

`inbox_items.kind` has a `CHECK`, which SQLite cannot alter, so the table is
rebuilt (as migration 19 rebuilt `thread_moves`), keeping every `seq` so
`inbox_fts` rowids still match:

```sql
CREATE TABLE inbox_items_26 (
    seq INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('reply', 'version', 'published', 'question', 'finished',
                                       'approval', 'connection', 'ended')),
    key TEXT NOT NULL UNIQUE,
    recipient TEXT,                              -- the person it is for; NULL: the owner
    artifact_id TEXT, thread_id TEXT, comment_id TEXT, version_n INTEGER,
    question_id TEXT, session_id TEXT, harness TEXT,
    request_id TEXT,                             -- approval
    grant_id TEXT,                               -- connection
    detail_json TEXT,                            -- finished, ended
    created_at TEXT NOT NULL,
    read_at TEXT
);
INSERT INTO inbox_items_26 (seq, id, kind, key, artifact_id, thread_id, comment_id, version_n,
    question_id, session_id, harness, detail_json, created_at, read_at)
    SELECT seq, id, kind, key, artifact_id, thread_id, comment_id, version_n,
           question_id, session_id, harness, detail_json, created_at, read_at FROM inbox_items;
DROP TABLE inbox_items;
ALTER TABLE inbox_items_26 RENAME TO inbox_items;
-- every index a list or read mark walks, led by COALESCE(recipient, ''):
CREATE INDEX inbox_unread ON inbox_items(COALESCE(recipient, ''), seq) WHERE read_at IS NULL;
CREATE INDEX inbox_by_recipient ON inbox_items(COALESCE(recipient, ''), seq);
-- … inbox_by_artifact, _kind, _harness, _session likewise …
-- by source, whatever the recipient: the newest stamp and a date range's
-- seq bounds (stamps follow seq for every recipient), a thread's items
-- (a move updates every recipient's), a question's item:
CREATE INDEX inbox_by_created ON inbox_items(created_at, seq);
CREATE INDEX inbox_by_thread ON inbox_items(thread_id) WHERE thread_id IS NOT NULL;
CREATE INDEX inbox_by_question ON inbox_items(question_id) WHERE question_id IS NOT NULL;
CREATE INDEX inbox_by_request ON inbox_items(request_id) WHERE request_id IS NOT NULL;
-- the connected people an item may go to:
CREATE INDEX viewers_people ON viewers(public_id) WHERE person = 1;
```

Keys of items for the owner are unchanged (`reply:<comment_id>`); an item
for another person appends `@<public ID>`, so one source makes one item per
recipient.

## 6. People, grants and tokens

### 6.1 Who a request speaks for: `Caller`

`Identity` (main spec §11) remains the one place that maps credentials, and
gives every handler one `Caller` (M8):

| Caller | Credential | Reader for the filter |
|---|---|---|
| `Owner {browser}` | an owner browser credential (owner cookie, events cookie, extension: `browser` true), or the daemon token on a request that names no session and carries no agent-channel marker (`browser` false): the CLI used by hand outside any harness, and scripts | none: not an agent channel (§8.5) |
| `OwnerAgent {session}` | the daemon token on a request that names a token session in its path (`/api/sessions/{id}/…`: the hooks, `clax feedback follow`) or in `X-Clax-Session` (the shim, Pi), or that carries `X-Clax-Channel: agent` (every agent-side client sends it, including `/mcp`'s internal client and the shim before its registration completes, and the CLI whenever it runs under a harness (§16: `CLAX_CHANNEL=agent`, which the plugins' wrapper exports, or a harness's shell variables); `session` none then). The marker counts only with the token | the owner |
| `PersonAgent {person, grant, session}` | `Authorization: Clax-Grant` with a valid signature (§6.6); `session` from `X-Clax-Session`, optional (§6.7) | the person (the owner when the grant's person is the owner row) |
| `Person {viewer}` | a `clax_viewer` cookie naming a connected, unrevoked person row | none |
| `Viewer {viewer}` | any other `clax_viewer` cookie | none |
| `Anonymous` | nothing | none |

- `PersonAgent` is never the owner (`is_owner()` false), even for a grant
  of the owner row: an owner's second machine acts as the owner's agents
  (its sessions have `person` NULL), never as the owner's browser.
- `Caller::browser()` is true for an owner browser credential and for
  `Person`; M20's routes require it together with `SameOrigin` (the
  viewer routes' `Origin` and `Sec-Fetch-Site` rule), so a page in a
  sandboxed or subdomain frame cannot decide anything with a person's or
  the owner's cookie.
- `Identity::person()` gives `Person::Owner` for `Owner`,
  `Person::Viewer(public_id)` for `Person`, and `Person::Agent` for the two
  agent callers. Person-scoped routes (questions, inbox, approvals, People)
  read it.
- No handler reads `Authorization` or checks the token itself (a guard
  test, §8.6). The db routes' level comes from `Caller`: for a
  `PersonAgent`, `rights.db_level(Y)` with the person as the viewer; agent
  replies are recognised from `Caller` (an agent caller with a session),
  never from the token's presence.

### 6.2 Invites

- `POST /api/people/invites` (owner, browser) `{name | viewer | me,
  rights?}` → `{invite: {id, command, expires_at}}`. `name` makes a new
  person row (no cookie: the browser link sets one later); `viewer` names
  an existing LAN viewer by public ID and marks it a person; `me` makes an
  invite for the owner row. The code is 12 Crockford base32 characters in
  three groups (60 bits), shown once; only its hash is stored. Expires in
  24 h. 409 `not_on_lan` when the daemon listens on loopback only. At most
  20 live invites.
- `GET /api/people/invites` (owner), `DELETE /api/people/invites/<id>`
  (owner, browser).
- The `command` is `clax connect http://<LAN address>:<port>/i/<code>`.

### 6.3 Grant requests

`POST /api/grants` takes a GNAP grant request:

```json
{
  "access_token": {"access": ["clax:collaborator"]},
  "client": {"key": {"proof": "clax-ed25519", "jwk": {"kty": "OKP", "crv": "Ed25519", "x": "<base64url>"}},
             "display": {"name": "sam-mbp"}},
  "interact": {"start": ["user_code"]},
  "clax": {"invite": "K7QFM2XD9PLA"}
}
```

The request is signed with the presented key (§6.6) and carries no token.

- **With `clax.invite`:** a live, unused invite binds the grant at once to
  its person, with the person's rights (`access` may only narrow them): 200
  with `access_token`, `continue` (for deletion), `subject` and
  `clax.browser_link`. An expired, used, revoked or unknown code is 400
  `invalid_invite` (one message for all four).
- **Without:** a pending grant, 201 with `continue {access_token, uri:
  "/api/grants/<gid>/continue", wait: 5}` and `interact.user_code`
  (`WXYZ-4821`: four letters without I and O, four digits). The owner's
  inbox gets a `connection` item. At most 3 pending requests per peer IP
  and 20 in all (429 `limit_reached`); a pending request expires after 10
  minutes.
- The owner decides with `POST /api/grants/<gid>/approve {user_code, name,
  rights?}` (browser, M20), which always makes a new person row named
  `name` (§11's rules for person names), or `POST …/deny`. A wrong
  `user_code` is 400 `wrong_code` and changes nothing; the third wrong code
  denies the request. A request cannot be approved into an existing person
  or the owner row (departure 9): those take an invite.
- The client polls `POST /api/grants/<gid>/continue` (signed, `GNAP`
  continuation token). Pending: 200 with `continue` (sooner than `wait`:
  400 `too_fast`). Approved: 200 with the access token, `subject`,
  `clax.person`, `clax.owner.display_name`, `clax.own_machine`,
  `clax.browser_link` and a new continuation token (an invite's 200
  carries the same). Denied or expired: 403 `denied`.
- On a grant whose tokens were issued, a continuation call with the body
  `{"clax": {"approve_owner": true}}` is `clax connect`'s yes to its
  question about the owner (M15): the person approves the owner's content
  everywhere (`created_via` `connect`, recorded as the person), 200
  `{"clax": {"owner_approval": {id, scope} | null}}` (null for the owner's
  own machine); 409 `not_issued` for a grant without live tokens. It is
  answered once, at connect: 409 `answer_closed` later than ten minutes
  after the tokens' issue, 409 `owner_answered` once the person has made or
  revoked any approval of the owner (so a revocation in People stands).
  `clax connect` refuses to run on an agent channel. Only the
  continuation token says it, never the access token an agent signs with.

### 6.4 Tokens, grants and people ending

- An access token is `cxg_` and 43 base64url characters (32 random bytes);
  a manage token `cxm_…`, a continuation token `cxc_…`; only SHA-256
  hashes are stored. Access tokens have no expiry and no rotation (M5); a grant unused for
  30 days expires with its tokens (`last_used_at` written at most once an
  hour).
- `DELETE /api/grants/<gid>/token` with `Authorization: GNAP <manage
  token>` (signed) revokes the access token and with it the whole grant,
  exactly as `DELETE <continue uri>` does: its sessions end (`revoked`)
  and release their threads (§14), and the key is free to connect again.
  There is no rotation (departure 11): a person who wants a fresh token
  disconnects and connects again.
- The daemon keeps an in-memory map of live token hashes to grant, loaded
  on start and updated on every change.
- Revoking a grant (the owner in People, the person's `clax connect --off`
  through `DELETE <continue uri>`, expiry) ends it and its tokens at once
  and ends every live session registered under it (§14).
- Revoking a person (owner, browser) revokes their grants and invites and
  sets `person_revoked_at`. Their viewer row, authorship and approvals stay
  as history. Their browser becomes `Caller::Viewer`: no inbox, People or
  approvals. Their content stays theirs (it is not `*unconnected`) and stays
  withheld from agents whose person never approved them.

### 6.5 The browser link

An approved grant response carries `clax.browser_link`:
`http://<host>:<port>/connect/b/<code>` (32 random bytes, one use, 10
minutes). `clax connect` opens it in the person's default browser. The
daemon answers with a page that sets the `clax_viewer` cookie to the
person's viewer and redirects to `/people`. A `clax_viewer` cookie the
browser already carried is replaced, never folded: the viewer it named
stays a separate LAN viewer with its own history, so a raced or sniffed
link cannot move anyone's history into the person. The link is refused for
the owner row and from a loopback peer naming a local host.

### 6.6 Key-bound requests (`clax-ed25519`)

Every request with an agent credential carries:

```
Authorization: Clax-Grant <access token>
Clax-Signature: t=<unix seconds>, n=<nonce: 22 base64url chars>, s=<base64url Ed25519 signature>
```

The signed message is the UTF-8 of

```
clax-ed25519 v2\n<METHOD>\n<path and query as sent>\n<Host>\n<t>\n<n>\n<SHA-256 hex of the body>\n<SHA-256 hex of the token>\n<SHA-256 hex of the agent headers>
```

The grant request signs with an empty token line; continuation and manage
calls sign with their `GNAP` token.

The last line covers the agent side's own headers, which pick the session
a grant acts as (and so its rights) and fill the audit journal (§20):
`X-Clax-Session`, `X-Clax-Channel`, `x-clax-via`, `x-clax-git` and
`x-clax-call`, in that order. Each contributes one record ending in `\n`:
`-` when the request does not carry it, else `+`, the value's length in
bytes in decimal, `:` and the value's bytes as sent. An absent header and
an empty one therefore differ (`-` and `+0:`). A value has no leading or
trailing spaces or tabs: HTTP parsers drop them, so the daemon hashes each
value as it reads it, without them, and every signer trims them from the
values it sends and signs. A request that carries one
of them more than once is 401 `invalid_signature`, whatever the values. So
`X-Clax-Session: 01J9ZK3M4N5P6Q7R8S9T0V1W2X`, `X-Clax-Channel: agent`,
`x-clax-via: mcp`, no `x-clax-git` and an empty `x-clax-call` hash

```
+26:01J9ZK3M4N5P6Q7R8S9T0V1W2X\n+5:agent\n+3:mcp\n-\n+0:\n
```

(`crates/clax-core/src/sig.rs`, `canonical_string_is_fixed`, pins this
vector). The daemon reads these values from the request it verifies, the
same request the handler then reads, before the body. `v2` is the only
version: every signer (the CLI's remote client, the shim and hooks in
remote mode, Pi) sends it, and the daemon accepts nothing else. Without
the line, an on-path peer could hold back a signed request and resend it
within its window, its nonce still unspent, naming another of the
person's sessions or rewriting what the journal records.

The daemon checks, in this order, before reading the body: the token's
hash in the in-memory map (401 `invalid_token`; `grant_revoked` for a
revoked grant's token), the header's shape (401 `invalid_signature`), `t`
within ±120 s of its clock (401 `stale_request`, with the daemon's `Date`),
and the nonce not yet seen for that key within the last 240 s (401
`replayed_request`). These header checks run in the outer layer that
`lib.rs`'s `wrap` applies, after the host rewrite, so they see every
request the daemon serves. The signature is then verified **before the handler runs, on
every request whatever its method**, in a `route_layer` that reads the
request's `MatchedPath` and takes the route's body limit from the same
`need!` declaration that names its right and class (`LIMIT`, beside
`RIGHT` and `CLASS`; M7), looked up by method and `MatchedPath` since one
path may carry several handlers with different limits, so there is no
second path matcher. The `route_layer` is applied in `routes::router`
after every `merge` and the shutdown route, so it covers every route. It
is a pass-through for every request without `Authorization: Clax-Grant`
(the daemon token, cookies, the extension): their bodies are never
pre-buffered. For a grant's request:

- a request with no body (no `Content-Length` or a zero one, and no
  `Transfer-Encoding`) is verified at once with the empty body's digest;
- any other request is read in full up to the route's `LIMIT` (413 beyond),
  hashed, verified, and only then handed to the handler with the buffered
  body;
- **unverified bytes in flight are bounded, with a bounded queue**: per
  key, one unverified body over 1 MiB is read at a time; others wait for
  the key's permit in a queue of at most 4 for at most 30 s (beyond either,
  429 `limit_reached`). Overall, unverified bytes share a 128 MiB budget: a
  body with a `Content-Length` reserves its length before any of it is
  read, waiting up to 30 s for room (then 503 `busy`); a chunked body
  reserves chunk by chunk and is refused with 503 `busy` when its running
  total cannot be reserved within 30 s;
- **unverified reads have a minimum rate and a deadline**: after its first
  10 s, an unverified body slower than 64 KiB/s on average is dropped with
  408 `timeout`, and so is one not read in full within 10 s plus 1 s per
  MiB of its declared length (of the route's `LIMIT` for a chunked body:
  about 106 s for a 96 MiB publish); either way its permit, allowance and
  budget are released, so a trickled body cannot hold them long;
- **small bodies are bounded per key too**: one key's unverified bodies of
  1 MiB or less hold at most 8 MiB in flight together (beyond, 429
  `limit_reached`);
- **a nonce is recorded only after its signature verifies**: the header
  check only tests for presence; after verification the nonce is inserted
  under the cache's lock with a second presence test, so of two copies of a
  valid request one passes and the other is `replayed_request`, and bad
  signatures spend no nonces. The per-key set is bounded at 65,536
  verified nonces per 240 s (429 `limit_reached` beyond);
- a failed verification is 401 `invalid_signature`. The first request of
  a grant (no token yet) is limited to 16 KiB.

The client re-signs and retries once on `stale_request` with the offset
from `Date`, and retries `limit_reached` and `busy` with exponential
backoff (1 s, 2 s, 4 s, with jitter; three retries), re-signing each
attempt. A request with an agent credential and an
`Origin` or a `Cookie` header is 403 `forbidden_origin`.

Keys: Ed25519 with `ring` (already in the lockfile, now a direct
dependency of `clax-core`), PKCS#8 in `<home>/remote.key` (0600). Signing
costs about 20 µs and verifying about 50 µs.

### 6.7 Session binding (M4)

- **Registration and refresh** (`POST /api/sessions`): the existing matches
  by `(harness, harness_session_id)` and by `(harness, parent_pid)` are
  restricted to rows with the caller's `grant_id` (`IS NULL` for the
  token). A grant's registration makes its row with `grant_id`, and
  `person` from the grant's viewer (NULL for the owner row).
- **Join** (`POST /api/sessions/join`, every hook): the same restriction,
  for every candidate it tries (harness session ID, parent PID,
  ancestors).
- **Listing** (`GET /api/sessions`, which hooks use to find their session
  by harness session ID): a grant sees only its own grant's sessions, and
  the token only the token's (so a hook never finds a grant's session by
  its harness session ID). The owner sees other machines' agents in People
  and the agents roster (§17), not in `clax doctor` or `clax feedback`.
- **Paths and headers:** every session ID in a path (`/api/sessions/{id}/…`)
  or in `X-Clax-Session` must name a session of the caller's grant (or, for
  the token, a token session): one of another credential is 403
  `foreign_session`. A session ID that names no session is 400
  `unknown_session` for a grant (the shim and Pi register again on it), and
  left to the handler for the token (today's answers). A grant's request
  whose path names one of its sessions acts with that session's rights,
  whichever of its sessions the header names. The check is in the
  `Agent<N>` extractor (M7), which reads the same `Path` as the handler.
- **Remote processes:** a grant's registration keeps no `pid`: it is a
  process of another machine, which the reaper must not probe as a local
  one (§14); its `parent_pid` stays, for matching within the grant.
- **Session-less grant requests** (the remote CLI): rights are `person ∩
  grant.narrow`, the reader is the person, and session routes are refused
  (400 `no_session`). The commands that work remotely are `clax list`,
  `read`, `open`, `versions`, `db …`, `comments <artifact>` (and `clax
  comments` without an artifact, which in remote mode lists through
  `GET /api/artifacts` and each artifact's threads instead of `GET
  /api/threads`, and takes the person's name from `remote.json` instead of
  `/api/viewers/me`), `comments resolve|reopen|send`, `agents` (the
  person's own agents, through `GET /api/agents`), `approvals` (listing the
  person's own, through `GET /api/approvals` and `GET
  /api/approvals/requests`), `status` (which remotely prints the remote
  connection, as `clax connect --status`, and the person's agents) and
  `connect`. `comments reply` without a session is 400 `no_session`: a
  remote person's comments are written in their browser, and agent replies
  come from agent sessions. The agent sides that run over the remote
  client (`mcp`, `hook`, `feedback`) and the commands about this machine
  itself (`serve` and `stop` for a local daemon, `init`, `uninit`, `bin`,
  `doctor` with its remote checks, `version`, `haiku`) run as usual.
  Every other command says "not available on a remote Clax" (exit 2).
- **Re-registration after an idle end** (§14): a registration with the
  same `harness_session_id` within the same grant, within 24 hours of that
  session's end with `ended_reason = 'idle'`, restores the direct watches
  saved in its `ended_watches_json` (still subject to rights and
  visibility at every take); no other end reason restores anything, and its first tool
  result carries a note: `[clax] Your session ended after <window> minutes
  without contact (<time>): N threads were released (<thread IDs>) and
  your questions withdrawn; your watches were restored.`, where the window
  is the one that ended it (30 for another machine's agent, 5 for this
  machine's). The rule is the same for both, and a watch comes back only
  where the session's rights still allow watching.

## 7. Rights

### 7.1 Access objects

```json
{"type": "clax-artifacts", "actions": ["read", "create", "update", "delete"], "locations": ["*"]}
{"type": "clax-comments",  "actions": ["read", "write", "reply", "resolve", "send", "watch"], "locations": ["*"]}
{"type": "clax-db",        "actions": ["read", "write"], "level": "interact", "locations": ["*"]}
{"type": "clax-ask",       "actions": ["ask"]}
```

Locations: `*`, `own` (artifacts the agent's person created:
`artifacts.person`), or artifact IDs. Everything outside this vocabulary is
never grantable (the route table, §7.5, marks it `Denied`).

### 7.2 Effective rights

```
effective(agent)                         = (person_rights ∩ grant.narrow ∩ session.narrow) ∪ session.elevate
effective(session-less grant request)    = person_rights ∩ grant.narrow
```

- `person_rights` is `clax:full` for the owner row and `viewers.rights_json`
  (or `clax:collaborator`) for a person.
- `∩` keeps an action at a location only when both sides allow it; `level`
  takes the lower. A missing narrowing is no narrowing.
- `∪` adds the owner's elevation, which may exceed `person_rights` and ends
  with the session (M2).
- `own` is resolved at check time.
- Rights are cached per session and follow the generation counter (M17).

### 7.3 Presets

| Preset | Rights |
|---|---|
| `clax:reader` | artifacts `read` `*`; comments `read`, `reply`, `watch` `*`; db `read` `*`; ask |
| `clax:collaborator` (default for a person) | reader, plus artifacts `create`, `update` `own`, `delete` `own`; comments `write`, `resolve`, `send` `*`; db `write` `*` at `interact`, and at `admin` on `own` |
| `clax:full` | every action at `*`, db `admin` |

### 7.4 Who changes what (all browser-only, M20)

- The owner sets a person's rights (`PUT /api/people/<public_id>/rights`)
  and elevates one agent (`PUT /api/agents/<handle>/elevate {access}`;
  `DELETE` removes it).
- A person narrows all agents of one of their machines (`PUT
  /api/people/me/grants/<gid>/narrow`) or one agent (`PUT
  /api/agents/<handle>/narrow`). A narrowing that would widen is 400
  `not_narrower`.
- No agent can change any rights: these routes are `Denied` to agents and
  refuse the bare token.
- Narrowing and elevation apply to agents of connected machines (a
  grant's sessions, the owner's other machines included). This machine's
  agents act with the daemon token, which rights do not limit (§21): 400
  `token_agent`.
- The owner ends any agent, and a person their own (`DELETE
  /api/agents/<handle>`, browser): the session ends with reason `person`
  and §14's cleanup.

### 7.5 The route table

Every route, with its class for agents. `Right` is checked at the artifact
the path names. **Filtered** routes run every author field of their
response, reads and writes alike, through `Visibility` (§8). **Own** routes
return only fields written by the caller's own person or Clax's own fields
(IDs, versions, counts, times, states). **Denied** routes refuse
`PersonAgent` callers (403 `forbidden`). **The `not_for_agents` rule:**
every Denied handler that builds a thread, comment, artifact, version,
document, question, inbox item, working or person view, or streams
events carrying them, answers `OwnerAgent` 403 `not_for_agents`; that
includes `/api/inbox*`, `/api/questions*` (the owner's list, answer and
release routes), `/api/live/site` and the other `/api/live/*` reads that
return thread views, `/api/events`, `/api/stream*`, `/api/people*` and
`GET /api/grants`. They serve the owner's browsers and the CLI used by
hand; agents reach the same content through Filtered routes (a live
page's threads through `comments_read`). So do the browser content
routes `/_blob/{asset}` and `/c/{aid}/v/{n}/…` and the artifact's shell
page (`/a/{aid}…`, whose bootstrap embeds the artifact, its versions and
threads). Denied handlers that build no such view (`/api/token` for the
shell, `/api/push`, `/healthz`, the gallery and static shell files)
serve owner agents as today. A guard enforces the rule (§8.6).
`GET /api/questions*` is also person-scoped: the owner sees the questions
of the owner's agents only, a person their agents' (§18).
**Grant** routes are the grant flow.

| Route | Class | Right |
|---|---|---|
| `GET /api/artifacts` | Filtered; artifacts the agent may not `read` are left out, and live pages for a caller that may not see them (a person's agent; the owner's agents list them, L10) (with `?artifact=`, answered as one that does not exist) | artifacts read |
| `POST /api/artifacts` | Filtered (returns the new artifact); a person's agent creates from a session (400 `no_session`), as its person | artifacts create |
| `GET /api/artifacts/{aid}` (embedded `working`, `participants`) | Filtered | artifacts read |
| `PATCH /api/artifacts/{aid}` (pin, metadata) | Filtered (returns the artifact); a person's agent edits from a session (400 `no_session`), never with `x-clax-via: page` (400 `invalid_via_page`) | artifacts update |
| `DELETE /api/artifacts/{aid}` | Own; a person's agent deletes from a session (400 `no_session`) | artifacts delete |
| `GET …/versions`, `GET …/versions/{n}` | Filtered (labels, notes, files, and `content_sha256` when a file is withheld) | artifacts read |
| `POST …/versions` (publish) | Filtered (returns the artifact and the new version, whose carried files keep their authors; on conflict the current version's summary, which clients read through `GET /api/artifacts/{aid}`); a person's agent publishes from a session (400 `no_session`), as its person, never with `x-clax-via: page` (400 `invalid_via_page`), and leaves untargeted feedback untargeted | artifacts update |
| `GET …/files`, `GET …/versions/{n}/files/{*path}` | Filtered (names; content 403 `withheld` with the authors of the version's withheld files, for a withheld file and, while the version withholds any, for a path it lacks) | artifacts read |
| `GET /c/{aid}/v/{n}/…`, `GET /_blob/{asset}` | Denied to grants (browser content routes; agents read through the routes above) | — |
| `GET …/assets` / `POST …/assets` / `DELETE …/assets/{id}` | Filtered (names, types, sizes) / Filtered (returns the asset; a person's agent uploads from a session, never with `x-clax-via: page`) / Own (a person's agent deletes from a session, 400 `no_session`) | read / update / delete |
| `GET …/presence` | Own (display names and states only) | artifacts read |
| `GET /api/artifacts/{aid}/working`, `GET /api/working` | Filtered (messages; the roster leaves out live pages for a caller that may not see them (a person's agent; the owner's agents keep them, L10) and, for a person's agent, artifacts it may not `read`, and names no other credential's session) | comments watch |
| `GET /api/threads` (all threads; `clax comments`, `/clax:comments`) | Filtered; Denied to `PersonAgent` (they list per artifact, §6.7) | comments read |
| `GET …/threads`, `GET …/threads/{tid}`, `GET …/threads/{tid}/clip` | Filtered | comments read |
| `POST …/threads` (create) | Filtered (returns the thread with its anchor); a person's agent writes from a session (400 `no_session`) | comments write |
| `POST …/threads/{tid}/comments` (incl. agent replies) | Filtered (returns the whole thread); a person's agent writes from a session (400 `no_session`) | comments reply (an agent reply) / write, and comments read |
| `POST …/threads/{tid}/resolve`, `…/reopen` | Filtered (returns the thread) | comments resolve, and comments read |
| `DELETE …/threads/{tid}` | Own; a person's agent deletes from a session (400 `no_session`) | comments resolve, and comments read |
| `POST …/threads/{tid}/send`, `…/threads:send` | Filtered (returns the threads) | comments send, and comments read |
| `GET/PUT/PATCH/DELETE …/docs/{*path}`, `GET …/docs`, `POST …/docs:batch`, `…/docs:str_replace`, `…/docs:acquire` | Filtered (every returned document; list `where`/`order_by` over visible documents only, §8.3); writes whose outcome depends on a withheld document's content are 403 `withheld` (with its withheld authors) before anything is evaluated, the pin included: `str_replace`, and every merge (`PATCH`, a batch `update`, `acquire` with `data`), whose success (its size check, which fields a delete marker removes) depends on the stored body; blind `PUT`, `DELETE`, batch `set` and `delete`, and `acquire` without `data` stay allowed, answer a withheld document's placeholder, and add the writer to its authors; a lease in force on a withheld document is busy whatever holder the caller names, so `acquire` never confirms another's holder or takes their lease over; a person's agent writes documents with or without a session (the remote `clax db …`, §6.7), never with `x-clax-via: page` (400 `invalid_via_page`) | db read (GET) / db write |
| `POST /api/sessions`, `POST /api/sessions/join` | Own (binding, §6.7) | none |
| `GET /api/sessions`, `GET/PATCH /api/sessions/{id}` | Own (the grant's own sessions) | none |
| `GET/PUT/DELETE /api/sessions/{id}/watches…` | Own | comments watch |
| `PUT/DELETE /api/sessions/{id}/live-watches` | Denied to grants (live pages) | — |
| `GET /api/sessions/{id}/feedback`, `…/notices` | Filtered (§8.4) | comments watch |
| `POST /api/sessions/{id}/feedback/ack` | Own | comments watch |
| `GET /api/sessions/{id}/working`, `POST …/working/renew`, `…/working/end`, `PUT/DELETE …/working/{aid}` | Own | comments watch |
| `POST /api/sessions/{id}/questions`, `GET …/questions/{qid}`, `…/withdraw`, `…/release`, `…/questions:terminal` | Own (questions and answers are the agent's person's) | ask |
| `POST /api/sessions/{id}/approvals`, `GET …/approvals/{rid}` | Own (the poll answers 404 for a request about an artifact the agent may not read) | ask |
| `POST /api/sessions/{id}/tool-calls`, `POST /api/sessions/{id}/tool-call-ids` | Own (the session's own calls; `produced` lists only that session's events, and an artifact the agent may not read is reported as none) | none |
| `POST /api/sessions/{id}/threads/{tid}/handoff` | Filtered (returns the thread; `owner.handle` only of the reader's own person's agents) | comments reply; with `to`, also comments send |
| `GET /api/agents` | Own (the caller's person's agents; other people's agents by name and state only) | none |
| `GET /api/approvals`, `GET /api/approvals/requests` | Own (the person's own approvals and requests; request views carry author, thread, count and state, never the withheld content; a person's agent's lists leave out those about an artifact its rights exclude) | none |
| `POST /api/grants`, `POST/DELETE /api/grants/{gid}/continue`, `DELETE /api/grants/{gid}/token` | Grant | — |
| `/mcp` (sessionless HTTP MCP, a `route_service`) | Denied to grants; for the owner's token its internal client sends `X-Clax-Channel: agent`, so its tools reach the handlers above as `OwnerAgent` without a session and are filtered for the owner | — |
| Everything else: `/api/token`, `/api/events`, `/api/stream*`, `/api/live/*`, `/api/sample*`, `/api/artifacts/{aid}/sample*`, `/api/artifacts/{aid}/room`, `/api/push`, `/api/extension*`, `/api/viewers*`, `/api/questions*`, `/api/inbox*`, `/api/people*`, `PUT /api/agents*`, changes to `/api/approvals*`, `GET /api/grants`, `/api/grants/{gid}/approve|deny`, `DELETE /api/grants/{gid}`, `/api/admin/shutdown`, `/connect/b/*`, the shell pages, `/healthz` | Denied | — |

Listing under location-limited rights leaves out what the agent may not
`read`. A thread write other than creating a thread (a comment or reply,
resolve, reopen, delete, send or batch send) also needs `comments read` at
the artifact (403 `not_permitted`): an agent acts only on threads it can
list. Creating a thread anchored on a file the agent may not see answers
as an anchor on a file the version lacks (400 `invalid_anchor`), so a
guessed name is never confirmed; `index.html`, which every version has,
is never refused. Feedback on an artifact the agent may no longer read is held as
withheld, with the placeholder "you no longer have access to this page".

## 8. The visibility filter

### 8.1 The question it answers

"May an agent of reader R see a field whose effective authors are A₁…Aₙ, at
artifact Y (thread T)?" Yes when, for every Aᵢ:

1. Aᵢ is R (M12); or
2. Aᵢ is Clax itself (placeholders, notes the daemon writes); or
3. R has a live approval for author Aᵢ (or, when Aᵢ is not the owner and
   not a connected person, for `*unconnected`) at scope `everywhere`, or
   `artifact` with Y, or `thread` with T (when the field is in a thread).

### 8.2 Effective authors, per field

| Field | Effective authors |
|---|---|
| A viewer comment's body | its writer (`author_public_id`; anonymous and pre-migration-12 unattributed comments count as `*unconnected`) |
| A page-written comment's body (`via_page`) | its writer and every author of the page at write time (`page_authors`: the authors of every file of its current version; none recorded: the owner) |
| An agent comment's body | the person of its session |
| A thread's anchor (quote, prefix, suffix, selector, custom name, file name, `html_hash`, an area's fingerprint) | the authors of the anchored file in the thread's version (`versions.file_authors_json`; the version's publisher when the file is unknown); none for an anchor on the whole artifact, which is on no page |
| A thread's `resolved_by` text; its batch note | the resolver (a viewer's public ID; for an agent's resolve the agent's person, `threads.resolved_by_person`; none recorded: `*unconnected`); the batch's sender (`send_batches.sender`; none recorded: `*unconnected`) |
| A clip | the thread's first commenter and the anchor's authors |
| A version's file (name and bytes) | that file's authors in `versions.file_authors_json`: for a file the version wrote, the publisher, and for a `by_page` version every author of the previous version's files; for a file it carried forward, the previous version's authors of that file |
| A version's label and note | its publisher (`versions.person`); for a `by_page` version also every author of its files |
| An artifact's title, description, icon and `capabilities` | each field's own authors (`artifacts.text_authors_json`): those of the publish or metadata change (`PATCH`) that last set it, a page's with every author of the page; a field never set since is its creator's (`artifacts.person`). A publish or change that leaves a field keeps its authors |
| An asset's name and bytes | its uploader (`assets.person`), and for a page's upload every author of the page at the upload (`page_authors`) |
| A document's fields | every `doc_authors` row for it (none: the owner) |
| A working record's message | its session's person |
| A question's text and answers | the asking agent's person (always R) |
| Names (people, agents, clients) | not filtered: names (§21.9) |

`person` NULL, `*owner` and the owner's public ID all normalise to `Owner`.
A live page's snapshots and anchors (owner-only, L10) count as the owner's.

**Document authors.** Every write (`PUT`/`set`, `PATCH`/`update`,
`str_replace`, `acquire` with data, batch writes) adds the writer to the
document's author set; the set only grows until the document is deleted,
which removes its rows. A page's write adds both the browser's viewer and
every author of the page (of every file of its current version, at write
time).
Touching, or rewriting, someone else's document never makes it yours.
Deleting it and writing a new one at the same path does (a visible,
destructive act that needs the delete right on it); §21.7 lists it.

### 8.3 Placeholders, per field

Withheld fields are replaced, never dropped:

- **Comment body:** `"body": null, "withheld": true` (the comment's ID,
  author name and person, and time stay).
- **Thread:** a withheld anchor has `quote`, `prefix`, `suffix`, `selector`,
  `custom_name`, `file` and `html_hash` null, its `area` without `tag`,
  `text` and `children`, and
  `"anchor_withheld": true`; a withheld clip has no `clip`, `clip_url` and
  `clip_path` null, `has_clip` false and `"clip_withheld": true`, and its
  route answers 403 `withheld` with the authors; a withheld `resolved_by`
  (with `resolved_by_name`) or batch note is null.
  The thread gains `"withheld": {"count", "authors": [{"public_id",
  "name"}], "approval": "none" | "requested" | "declined"}` whenever any
  field of it is withheld.
- **Artifact:** each of `title`, `description`, `icon`, `capabilities`
  null when its own authors are withheld, and `"withheld": {"authors":
  […]}` naming every author withheld; embedded working messages null
  with `"withheld": true`.
- **Version:** `label` and `note` null when withheld; each withheld file
  left out of `files`, with `"files_withheld": n`, and then
  `content_sha256` (a hash over every file's name and bytes) null; its
  content route answers 403 `withheld` with the authors of the version's
  withheld files, alike for a path the version lacks while it withholds
  any file, so the route never confirms a guessed name.
- **Document:** read by its path, `{"doc_id", "version", "withheld":
  {"authors": […]}}`, no `data` (the caller named the path); a write by
  its path answers the same placeholder, and its `created`, `deleted`,
  pin conflicts and batch results (`op`, `path`, `version`, `deleted`)
  say no more than it does: that the named path holds a document, and
  its version. In `GET
  …/docs`, withheld documents are counted, never listed: only visible
  documents are returned, evaluated, matched and ordered, and the response
  carries `"withheld_count": n` (the collection's documents left out,
  whatever the filter, cursor and limit; never one the `db` rules
  already hide from the caller), so neither their paths nor their field
  values are revealed; pages and `next_cursor` run over visible documents
  only, and so does the byte budget of an ordered query without a limit.
  A response to a caller whose view withholds nothing (the owner by
  hand, a person's browser, a single-owner install) carries no
  `withheld_count`; an agent whose view can withhold gets it, `0` when
  nothing is withheld. Content-dependent writes on a withheld document are 403
  `withheld` before evaluation (§7.5).
- **Asset:** name null, and `content_type`, `ext` and `size` (which its
  bytes decide) null, `"withheld": true`; its ID, artifact and time stay.
- **Feedback text** (§8.4): for a withheld comment body, one line per
  (author, thread): `[clax] N comments from <name> on "<title>" (thread
  <ID>) are withheld until your person approves them.` then `Your person
  has been asked.` or `To ask your person, call ask with {"approval":
  {"thread_id": "<ID>"}}.`; `<title>` is `an artifact by <name>` when the
  title is withheld. In a delivered comment's block: `Anchored on:
  (withheld: page by <name>)`, `Clip: withheld`, the title as `an artifact
  by <name>`, a withheld batch note as `(note withheld)`.

### 8.4 Delivery

- `take_feedback` (every tier: piggyback, stop hook, prompt hook, wait,
  inject, queue) partitions the session's eligible rows by the comment
  body's visibility. Rows with a visible body are delivered as today, each
  `FeedbackItem` field filtered on its own (title, anchor, clip path,
  batch note). Rows with a withheld body are left undelivered; those
  without `placeholder_at` produce placeholder lines and get it set.
- Each take reads the generation counter (M17) and rebuilds `Visibility`
  when it changed since the request began; every change M17 lists bumps it
  and wakes the affected sessions' feedback and notices waiters. A
  long-poll registers its waiter first, then takes, then waits: a bump or a
  new row after the waiter registered wakes it, so none is missed.
- Notices (Grok's monitor, Claude Code's channel) announce only rows with a
  visible body; their `title` is filtered like the payload's.
- `codex queue` sends only the filtered payload of visible rows.
- Resends apply to rows with visible bodies only, re-filtered at resend.
- A thread's `feedback_state` shown to people says `withheld` (with the
  approver) for held rows.

### 8.5 Agent channels

The filter runs for `OwnerAgent` and `PersonAgent` callers (§6.1): the
shim's and Pi's tools, the hooks (stop, prompt, session start), `clax
feedback follow`, every feedback tier and notice, the sessionless `/mcp`,
every `clax` command run under a harness (the plugin slash commands
through the wrapper, such as Claude Code's `/clax:list` and
`/clax:comments`; an agent's own `clax` calls through its shell tool;
Claude Code's `!clax …` lines, whose output goes into the model's
context), Pi's `/clax` command (through its `DaemonClient`, which sends
the marker), and the remote CLI. A CLI run under a harness is an agent
channel, detected as §16 says. It does not run for people: the owner's
browsers and the CLI run outside any harness (or with
`CLAX_CHANNEL=owner`), connected people's browsers, and LAN viewers see
what they see today.

What this is: a guarantee about what Clax hands an agent through its agent
channels. What this is not: an access-control boundary (§21). An agent can
still fetch a public page as an anonymous LAN viewer with a web tool, and
an owner's local agent with shell access can run the CLI by hand, read
`~/.clax`, or fetch the owner's browser credential. The skills
tell agents not to open other people's Clax pages with web or browser
tools, and to read and act through the Clax MCP tools rather than the
CLI. Agent payload URLs stay as they
are (people open them).

### 8.6 Guard tests

- **Route guard** (`routes/mod.rs`, `mod agents`, beside `mod l10`): reads
  every route (and the `/mcp` service) from source, and requires each
  handler's signature to carry exactly one of `Agent<N>` (whose `N` names
  the route table's right and class), `DeniedToGrants`, or `GrantFlow`; a
  `Filtered` handler's source must pass its response through `vis.filter_`,
  or, for the feedback and notices polls, hand over what a filtered store
  take built (`Store::take_feedback_readable`, `Store::take_notices`, which
  take the session's `Visibility` and decide held, told and delivered in
  their own transaction, which no pass over their result could);
  an `Own` handler must not build a thread, comment, artifact, version,
  document or working view. The route table in §7.5 is generated into
  `docs/contract.md` from the same declarations, so they cannot drift.
- **Wrapper guard:** every command and hook a plugin runs (Claude Code's
  `commands/*.md` `!` lines and `hooks/hooks.json`, the Codex and Grok
  plugins' `hooks/hooks.json`, Pi's spawned commands) invokes `clax`
  through `ensure-clax.sh exec`, directly or through `tool-hook.sh` (which
  the guard follows: it must itself call `ensure-clax.sh exec hook`). Pi
  resolves the binary once through the wrapper and then spawns it directly
  for exactly two commands whose output never reaches the model, `serve
  --json` and `status --json`; the guard allowlists those two by name, and
  Pi sets `CLAX_CHANNEL=agent` in their environment anyway. Pi's `/clax`
  command goes through its `DaemonClient`, which sends the marker. The
  test also checks every copy of `ensure-clax.sh` is identical and exports
  `CLAX_CHANNEL=agent` for every mode but `serve`, `stop`, `doctor` and
  `init`.
- **`not_for_agents` guard:** every handler whose signature carries
  `DeniedToGrants` and whose source builds a thread, comment, artifact,
  version, document, question, inbox, working or person view (the same
  textual check as the Own guard), or streams events, must also call
  `caller.refuse_agents()?`; the test fails otherwise. The route table's
  Denied rows record the result.
- **No inline credentials:** no handler outside `identity.rs` and `auth.rs`
  calls `has_token` or reads `Authorization`.
- **Delivery guard** (`clax-core`): `FeedbackItem`, `Notice` and
  `Placeholder` are built only inside functions that take `&Visibility`,
  and no non-test code of `clax-core` or `clax-server` names
  `Visibility::all()`; a test scans the source.
- **Canary sweep** (`clax-server/tests/api_canary.rs`): a seed puts a
  distinct canary string, authored by Mia, in every author field of §8.2
  (title, description, icon, label, note, file names and bytes, asset name
  and bytes, document fields, comment body, anchor quote, prefix, suffix,
  selector, clip, batch note, working message, `resolved_by`,
  `capabilities`, a document path, and a file the owner's agent carried
  forward from Mia's version). For every route of the route table
  (generated from the declarations; reads, writes and error paths), called
  over HTTP as Sam's agent and as the owner's agent, and for every agent
  channel driven through its real client (the MCP shim's tools, `/mcp`'s
  tools, the `clax hook` binary's stop, prompt and session-start hooks over
  `clax-cli`'s client, `clax feedback follow`, Pi's extension, and the
  context of every plugin slash command, run through the real wrapper:
  Claude Code's `/clax:*` commands' `!` lines and Pi's `/clax` command;
  the Codex and Grok plugins have no slash commands; and the plain `clax`
  binary run with each harness's shell variables of §16 and no wrapper),
  with Mia connected and not approved, no output byte contains a canary.
  The HTTP part also calls every Denied route as the owner's agent (each
  answers `not_for_agents` or carries no canary), including `GET
  /api/threads` (Filtered), `/api/live/site` and `/api/questions`; the seed
  includes a live-site thread and a question asked by Mia's agent. After an approval at each
  scope, each canary appears where the scope covers it.
- **Behaviour matrix** (`api_visibility.rs`): approval scopes, revocation,
  generation changes during a long-poll.

### 8.7 Cost

`Visibility::for_reader` reads the reader's live approvals once (one
indexed query, typically under ten rows) into hash sets; each field check
is a few hash lookups; the session-to-person map is cached in `AppState`.
**Fast path:** while the install has no person rows and the `*unconnected`
approval is live, `Visibility` for the owner is `AllVisible` (one atomic
load), so a single-owner install pays nothing (§23).

## 9. Approvals

### 9.1 Objects

An approval says "agents of `approver` may see `author`'s content at
`scope`". A request asks the approver for one.

### 9.2 Requests

- **Routing (M18):** a send naming an agent (the picker, `to`, `@name`,
  handoff, a batch naming one, or a page's `sendToClaude` naming one)
  whose person has not approved some author of the thread's comments
  makes (or joins) a request per such author, `cause: routing`. A send
  naming no agent (Send with no agent chosen, `@agent`) and a later
  comment on a sent thread only join open requests there. A page's send shows on the card as "<publisher>'s
  page, via <viewer>'s browser", and its authors include the publisher.
- **Asking:** `ask` with `approval: {url_or_id?, thread_id?, author?}`
  makes (or joins) a request for each author withheld from the session
  there, `cause: ask`, and waits for the decision (same timeout and
  `call_again` as questions). Returns `{"approval_id", "status":
  "approved" | "declined" | "open", "scope"}`. Nothing withheld: 400
  `nothing_withheld`.
- **Batching (M19):** one open request per (approver, author, artifact,
  thread); new withheld content there raises `count` and re-announces the
  one inbox item.
- **Limit:** 100 open requests per approver, and 20 per (approver,
  author), so no one author spends an approver's requests on everyone
  else; beyond either, `ask` is 429 `limit_reached`, routing holds the
  content without a request, and People shows one banner: "More is
  waiting for your approval than the inbox lists: review People". The
  banner stays until no routed content it counted is held without a
  request (each later decision rechecks).
- An open request is withdrawn when its thread is resolved or deleted, its
  artifact deleted, (for `cause: ask`) the asking session ends, or its
  approver or author is revoked as a person.

### 9.3 Deciding

`POST /api/approvals/requests/<id>/decide {scope: "thread" | "artifact" |
"everywhere" | "decline"}` from the approver's browser (M20; 404 for anyone
else, 403 `browser_required` for the token). Approving makes the approval
(or finds the live one), marks the request `approved`, closes the
approver's other open requests for that author the scope covers, bumps the
generation (M17), and wakes the affected sessions. Declining keeps the rows
held; a later `ask` there makes a new request only after 10 minutes (400
`recently_declined` before).

Approvals made directly (People → person → "Always allow") need no request.

### 9.4 Revoking

`DELETE /api/approvals/<id>` (the approver's browser). From the next take
or read on (M17), content the approval covered is withheld again unless
another live approval covers it: undelivered rows are held again (their
`placeholder_at` cleared so the agent is told once), reads show
placeholders. What was delivered stays delivered. Revocation never ends a
session or releases a thread.

### 9.5 Where people decide

- **Inbox** (`approval` items, unread until decided): the card shows the
  author, the agent, the page and thread, the withheld content itself
  (people are not filtered), the count, and the four buttons; the thread
  button says "this thread, including later replies" (R1).
- **People** (`/people`): per person, "Their content reaching your agents"
  (live approvals, with Revoke) and "Waiting".
- **The extension's panel** (owner only) shows no approval cards: an
  approval item in its Inbox tab links to the shell's inbox, where it is
  decided.
- **CLI:** `clax approvals [--open]` lists; deciding, allowing and revoking
  print the People link (M20).

### 9.6 The `*unconnected` switch (O6)

People shows the owner: "Comments from people on your LAN who are not
connected reach your agents without asking. This includes connected people
commenting from a browser or script that is not signed in as them." with a
switch, on by default. Off revokes the approval; on makes a new one. It is
shown again beside the first invite. In every payload and view an agent
gets, author lines for unconnected viewers read `<name> (not connected)`.
That includes history whose author was never recorded, which the filter
counts as `*unconnected` (§5.2, §8.2): comments from before viewers had
public IDs (migration 12), comments from a browser identity never claimed
for the owner, and batches from before senders were recorded (migration
25), even when the owner wrote them. Matching a name to the owner would let
any viewer who typed it pass as the owner (§21.2).
Display names may use any script, and any mix of scripts. Protected names
are the owner's, each connected person's and each live agent's. A display
name is 409 `name_taken` when it looks like a protected name other than its
holder's own: when they share a UTS #39 confusable skeleton, taken of the
name's token form (case and diacritics folded, invisible and
default-ignorable characters dropped, the right-to-left and left-to-right
marks among them, spaces narrower than U+0020 such as the thin and hair
spaces dropped, other whitespace and U+2800 as one space) and of the token form
of the skeleton of the name as written. So `Аlex` with a Cyrillic `А`,
`AIex`, `Ａｌｅｘ` and `Alex` with a Hangul filler collide with `Alex`, and the
all-Cyrillic `Сосо` with `Coco`. A name holding a bidirectional embedding,
override or isolate control (U+202A–U+202E, U+2066–U+2069) is 400
`invalid_name`. A new person also cannot be named after a viewer who was
never invited and is not revoked (409 `name_taken`, saying to invite that
viewer instead); a revoked person's or a lapsed invite's name is free
again.

## 10. Routing to another person's agent

- The Send menu lists every live agent that may receive the thread: "Your
  agents", then each person's ("Sam's agents"). An agent of another person
  who has not approved this thread's authors is listed with "Sam approves
  first".
- Sending to it (button, `to`, `@name`) stores the send and makes the
  routing request (§9.2). The thread's delivery state reads "waiting for
  Sam's approval".
- A send to an agent that may not `read` the artifact is 400
  `agent_cannot_read`.
- A page's `@name` sends nothing (as its `@agent` sends nothing today); a
  page's `sendToClaude` routes as above.

## 11. Agent names

- **Derivation.** On registration the agent side sends `repo` (the name
  of the repository's common git directory, as `git rev-parse
  --git-common-dir` gives it: the directory holding that `.git`, found
  from the nearest directory above `cwd` holding `.git`, a directory or a
  worktree's file; a worktree is named after its repository, not its own
  directory) and `branch` (read from that checkout's `HEAD`; detached is
  none), with no `git` process. The daemon names the session
  `<repo>` when the branch is none, `main` or `master`, else
  `<repo>-<last path segment of the branch>`; lowercased, every character
  other than `[a-z0-9._-]` made `-`, runs of `-` collapsed, cut to 32.
  Taken: `<base>-<harness>`, then `<base>-2`, `-3`, … (R3). No repository:
  the harness.
- **Task.** The agent sets a name with `status {"name": "fix-chart-axis"}`
  (409 `name_taken`; 400 `invalid_name`, which includes a chosen name
  ending in `.`, `_` or `-`), again later unless its person renamed it
  (409 `name_locked`).
- **Rename.** The agent's person (and the owner, for any agent) renames it
  in the roster or People, or `clax agents rename`; `PUT
  /api/agents/<handle> {name}` (a browser, or the token from the CLI;
  renaming is not a right and changes nothing an agent can see beyond the
  name).
- **Collisions and reserved names.** Unique among live sessions. Reserved:
  `agent`, `owner`, `clax`, `me`, `everyone`, and every connected person's
  and the owner's display name in token form (lowercased, diacritics
  removed, spaces as `-`). Naming or renaming a person to a name whose
  token form is a live agent's name is 409 `name_taken`; so is the owner
  taking a live agent's or a connected person's name.
- **`@name`.** `@` followed by a live agent's name as a whole token (start
  or a non-alphanumeric before; end, whitespace or punctuation other than
  `.`, `_`, `-` after, where a sentence's closing full stops are not part
  of the name: `@web.` mentions `web`, `@web.dev` does not) sends the thread to that agent (a send with `to`),
  giving it a direct watch when it has none. A name that names no live
  agent sends nothing; the response carries `unresolved_mentions`. `@agent`
  keeps its meaning.
- **Display.** Agents show as their name, with their person wherever more
  than one person's agents appear. A reply's author line in payloads is
  `<name> (<person>'s agent, via <harness>)`, or `<name> (agent, via
  <harness>)` for the reader's own person's agents.

## 12. Thread ownership and handoff

- **Owner.** `threads.target_session_id` (M23). Set by a send with `to`,
  `@name` or a handoff (explicit: they route, §10), or by an implicit
  take. O8 decides first take among one person's agents, so an implicit
  take competes only among the agents of the person who sent the comment
  taken: the writer of a viewer comment when the writer is a connected
  person, else the owner (the owner, LAN viewers who are not connected
  and anonymous viewers reach the owner's agents, O6). An implicit take is
  the first `take_feedback` (or in-band acknowledgement) that delivers one
  of the thread's rows to such an agent, or the first reply by such an
  agent that sees one of those comments: in that transaction the session
  becomes the owner, the undelivered rows of that person's other agents
  for the thread are deleted, and those agents' open `ask` requests for the
  thread are withdrawn (`approval.withdraw`, reason `thread_taken`). Other
  people's agents' rows stay. A placeholder take, a delivery to another
  person's agent, and a reply by another person's agent or by an agent
  that sees none of the thread's comments make no owner.
- **Views.** Thread views carry `owner: {handle, name, person, live}`,
  with `handle` only for the owner (by hand or in a browser) and for
  readers of the owning agent's own person (§13: other people's agents by
  name and state only); an agent reading a thread owned by another live
  agent sees `"taken_by": "<name>"`.
- **Refusals (M24).** An agent reply or resolve on a thread whose owner is
  another live session: 409 `thread_taken` with the owner's name (in the
  message and in `taken_by`). Threads with no owner, or an ended owner,
  accept any agent with the rights; the first reply takes the thread only
  as an implicit take above.
- **Handoff.** `comments_handoff {url_or_id, thread_id, to}` by the owner
  agent; while the thread has no owner, by an agent that could take it
  implicitly (above). (An owner's end releases its threads, §14, so a
  thread never keeps an ended owner.) A
  handoff with `to` also needs `comments send`. `to` names, by name or
  handle, a live agent that may read and watch the artifact, which is
  given a watch when it has none (as `@name` gives), only once nothing
  refuses the handoff; the thread is sent to it (§10, including the
  routing request): it becomes the owner, other sessions' undelivered rows
  of the thread are deleted (their sessions' open `ask` requests for it
  withdrawn, reason `thread_taken`), and it gets a row for every viewer
  comment it has none for; the result is the filtered thread with its new
  owner. Without `to`, the live owner releases the thread (owner cleared,
  undelivered rows untargeted, one per comment); a release of a thread
  with no live owner changes nothing. Anyone else: 409 `thread_taken`.
- **People** take over by sending again. A send naming one agent (the
  picker, `to`, a batch naming it) that makes it the new owner removes the
  previous owner's undelivered rows of the thread and withdraws that
  session's open `ask` requests for it (`approval.withdraw`, reason
  `thread_taken`), as a take does. `@name` adds the named agents and
  keeps the earlier ones' rows (§11).

## 13. Agent-aware attention

- **`GET /api/agents`** (the owner sees every agent, a person their own;
  browser or the CLI's token) → `{agents: [{handle, name, person, harness,
  project, live, state, since, blocked: {since, questions, approvals} |
  null, threads_owned, working: [{artifact_id, title, message}],
  unread}]}`. `state`: `waiting` (an open question or `cause: ask`
  request), `working`, `idle`, or `ended` (within the last hour). `project`
  only to the agent's person and the owner.
- **The `agents` stream topic** (person-scoped) announces changes.
- **Inbox grouping.** `/inbox` opens with an agent strip, one chip per
  agent with unread items or a block, blocked-longest first: "clax-claude ·
  waiting on you 12 min". A chip filters the inbox (`agent=<handle>`).
  `GET /api/inbox?group=agent` returns the strip's data.
- **Roster.** The gallery's roster, the artifact view's agent list and
  `clax status` show the same states and block times.
- **Notifications** title an approval request "Mia wants to reach
  clax-claude".

## 14. Lifecycle cleanup

When a session ends, in the end's transaction, its `ended_reason` is
written: `exit` (its end hook or its shim's exit), `idle` (the reaper),
`revoked` (its grant or token revoked) or `person` (its person or the
owner ended it). An `idle` end also saves its direct watches to
`ended_watches_json` before they are deleted (§6.7). Then:

1. Open questions are withdrawn (as today).
2. Open `cause: ask` requests of this session are withdrawn; `cause:
   routing` requests stay.
3. Threads it owns are released.
4. `narrow_json` and `elevate_json` are cleared; the name is freed.
5. Watches and working records end (as today).
6. When it owned threads or had open questions or `ask` requests, one
   `ended` inbox item for its person (M25): "clax-claude ended · released 2
   threads · withdrew 1 question".
7. Send menus and pickers drop it; the agent strip keeps it as `ended` for
   an hour.

Remote sessions (with a `grant_id`) are never PID-probed; they end after 30
minutes without a heartbeat (local sessions keep 5 minutes), counted only
while the daemon runs: a remote session's unseen time starts no earlier
than the daemon's start or its last wake from sleep, so a laptop
that sleeps briefly keeps its sessions; a longer sleep releases its threads
and makes the note, and the agent's next request registers anew, getting
its watches back and a note saying what ended (§6.7).

## 15. Contract summary

The route table (§7.5) is the agents' contract. New routes:

- Grant flow: `POST /api/grants`, `POST|DELETE /api/grants/<gid>/continue`,
  `DELETE /api/grants/<gid>/token`.
- Owner (browser for changes, M20): `POST|GET /api/people/invites`, `DELETE
  /api/people/invites/<id>`, `GET /api/grants?status=`, `POST
  /api/grants/<gid>/approve|deny`, `DELETE /api/grants/<gid>`, `GET
  /api/people`, `PUT /api/people/<pid>/rights`, `DELETE /api/people/<pid>`,
  `PUT|DELETE /api/agents/<handle>/elevate`.
- Person (owner or connected person, own items; browser for changes): `GET
  /api/people/me`, `PUT /api/people/me/grants/<gid>/narrow`, `PUT
  /api/agents/<handle>/narrow`, `PUT /api/agents/<handle>` (rename),
  `DELETE /api/agents/<handle>` (end; the owner ends any agent), `GET
  /api/agents`, `GET /api/approvals`, `POST /api/approvals`, `DELETE
  /api/approvals/<id>`, `GET /api/approvals/requests`, `POST
  /api/approvals/requests/<id>/decide`; the question and inbox routes,
  scoped to the caller's person; the `agents`, `questions`, `inbox` and
  `people` topics, scoped likewise.
- Agent session routes: `POST /api/sessions/<sid>/threads/<tid>/handoff`,
  `POST /api/sessions/<sid>/approvals`, `GET …/approvals/<rid>?wait=`.
- Browser: `GET /connect/b/<code>`; shell page `/people`.
- The extension gateway admits `GET /api/approvals/requests` for the owner
  (the panel's count and links); deciding happens in the shell.
- `GET /api/agents`, `GET /api/approvals` and `GET /api/approvals/requests`
  also serve a person's agents and remote CLI (Own, §7.5).

## 16. MCP, CLI and plugins

- **Tools.** `status` gains `name` and reports `name`, `person`, `rights`
  and `remote`. `ask` gains `approval` and `approval_id`. New
  `comments_handoff`. New error codes: `not_permitted` (with the right),
  `withheld`, `thread_taken`, `foreign_session`, `name_taken`,
  `name_locked`, `invalid_name`, `nothing_withheld`, `recently_declined`,
  `agent_cannot_read`. Twenty-five tools.
- **Clips** for a remote session: the payload's `Clip:` line names
  `clax-clip:<artifact>/<thread>`; the agent side fetches it (signed) into
  `<home>/remote-cache/clips/` and rewrites the line; a failure rewrites it
  to `Clip: unavailable (<reason>)`.
- **`clax connect`**: `<invite URL>`, `<daemon URL>`, `--status`, `--off`,
  `--url <new daemon URL>`, `--name <client name>`,
  `--no-browser`, `--yes`. Writes `remote.json` (0600) and `remote.key`
  (0600). URLs name the daemon by IP literal. An ad-hoc request waiting
  for the owner is kept in `remote-pending.json` (0600): a connect that
  dies resumes it on the next run against the same daemon, and connecting
  elsewhere or `--off` cancels it (`DELETE <continue uri>`); Ctrl-C and the
  ten-minute limit cancel it at once, so a lost connect never holds its
  key or its peer's request slot until the request lapses. A request whose
  tokens were issued to an answer that never arrived cannot be finished:
  the command says so and asks the person to have the owner revoke it.
  Off a terminal, without `--yes`, the question about the owner goes
  unanswered and nothing is approved.
- **Remote mode** (M26): the clients of `clax-cli`, `clax-mcp` and Pi read
  `remote.json` when present (and `CLAX_REMOTE` is not `0`): no daemon
  discovery or auto-start, every request signed and marked
  `X-Clax-Channel: agent` when it comes from an agent-side client. `clax serve` still serves a local daemon for local
  use.
- **`clax people`** (owner): lists people, machines and agents. **`clax
  agents`**: list, `rename`. **`clax approvals`**: list. Invites, rights,
  elevation, narrowing and approval changes print the People link (M20).
- **The agent-channel marker.** The shim, `/mcp`'s internal client, every
  `clax hook` command, `clax feedback follow` and Pi send `X-Clax-Channel:
  agent` on every request, local or remote. The CLI sends it when it runs
  under a harness, which it decides from its environment, first match
  wins:

  | Variable | Set by | Meaning for the CLI |
  |---|---|---|
  | `CLAX_CHANNEL=owner` | the owner, explicitly | not an agent channel (the owner's own scripts) |
  | `CLAX_CHANNEL=agent` | the plugins' wrapper `ensure-clax.sh exec` (every mode but `serve`, `stop`, `doctor`, `init`); Pi for its direct spawns | agent channel |
  | `CLAUDECODE=1` | Claude Code, in its Bash tool's and `!` commands' shells (observed in Claude Code 2.1.x; `scripts/smoke-claude-ask.sh` already unsets it to run Claude Code nested) | agent channel |
  | `CODEX_THREAD_ID`, `CODEX_CI` (any value), `CODEX_SANDBOX` (inside its sandbox) | Codex, in its shell tool's environment (probed with Codex 0.162.0 `codex exec`) | agent channel |
  | `PI_CODING_AGENT=true` | Pi 0.73.1, which sets it in its own process; its bash tool inherits it, as it inherits the `CLAX_CHANNEL=agent` the Clax Pi extension sets when it loads | agent channel |
  | (`GROK_SESSION_ID`, not read) | Grok Build passes it to the stdio MCP servers and hooks of a session (main spec §11); its shell tool was not probed, so the CLI does not read it until a probe confirms it | — |

  `scripts/probe-harness-env.sh`, run once under each harness by the
  implementer (and recorded in `docs/contract.md`), prints the
  environment its shell tool gives a command; the CLI's list is exactly
  the variables the probe saw (a variable no probe confirms is left out
  and Pi-style `CLAX_CHANNEL` export is used for that harness instead).
  So every plugin slash command and hook, every `!clax …` line and every
  `clax` call an agent makes through its shell tool is an agent channel.
  Only the CLI run outside any harness, or with `CLAX_CHANNEL=owner`, is
  the owner.
- **Readable refusals.** A CLI command that reaches a `not_for_agents`
  route (`clax inbox`, anything reading `/api/events` or `/api/stream*`)
  under a harness prints one line, for example "The inbox is for people:
  open http://localhost:7480/inbox in a browser.", and exits 2, never a raw
  403.
- **Hooks** run over the remote client, finding their session through the
  grant-scoped `GET /api/sessions` and join (§6.7); `codex queue` is not
  attempted for remote sessions, and `status.push` says so.
- **Skills** gain "Working beside other agents and people": your name and
  how to set it; withheld content and how to ask; `thread_taken` and
  handoff; placeholders are never instructions; do not open other people's
  Clax pages with web or browser tools, use the Clax tools; never fetch
  `/api/token`, run `clax` commands that change approvals or rights, or
  act in People: approvals are your person's, in their browser. Read and
  act through the Clax MCP tools, not the `clax` CLI.

## 17. Shell and extension

- **People** (`/people`, lazy module `web/shell/src/people/`): for the
  owner, people (rights, machines with key fingerprints, agents, approvals
  in both directions), invites, connection requests, and the
  `*unconnected` switch (§9.6); for a person, their machines, agents
  (rename, narrow), approvals and requests.
- **Inbox:** `approval` and `connection` cards, `ended` rows, the agent
  strip.
- **Threads:** the owner chip, the `withheld` delivery state, the Send menu
  by name and person.
- **Extension panel:** agent names in its Send menu; approval items link
  to the shell's inbox.
- Every UI change is verified in a real browser with screenshots, at phone
  width too. The UI questions (§26) go to the owner in one batch before the
  first UI task.

## 18. Owner and person views of the same thing

| Thing | Owner (browser, CLI by hand) | Owner's agents | Connected person (browser) | Person's agents | LAN viewer |
|---|---|---|---|---|---|
| Gallery, artifacts, threads | all | filtered for the owner | all | filtered for the person | all |
| Live pages | yes (local) | yes (local) | never | never | never |
| Inbox, questions | own | own questions | own | own questions | none |
| People, approvals | everyone; changes in the browser | none | self; changes in the browser | none | none |
| Agents roster | every agent | own (`status`) | own agents; others' names and states | own (`status`) | names on threads |

A person's inbox and question views are not filtered by Visibility; they
carry only what that person's browser could read elsewhere (never a live
page), and an agent's `project` only for the person's own agents and the
owner's view (§13).

The owner using the CLI by hand outside any harness (the token, no
session, no marker) is not filtered: it is the owner, not an agent
channel. The same CLI run by a harness through the plugins' wrapper is.

## 19. Migration of single-owner installs

After migrations 24–26, with no person ever connected, these change and
nothing else:

1. **Agent names.** New sessions get derived names, shown where the shell
   showed the harness ("Send to clax-claude"), in reply author lines
   (`clax-claude (agent, via claude)`), and in `taken_by`.
2. **`@<name>`.** A comment with `@` followed by a live agent's name (for
   example `@web`, when an agent works in a repository named `web`) now
   sends the thread to that agent and gives it a watch.
3. **First take (O8).** With several of the owner's agents watching one
   artifact, an untargeted send goes to the first agent that takes it.
4. **`thread_taken` (M24).** The owner's second agent cannot reply to a
   thread the first owns.
5. **`ended` notes (M25)** in the owner's inbox.
6. **Additive JSON fields:** `owner`, `taken_by`, `name`, `person`,
   `withheld` (absent or null), `rights` in `status`.
7. **`(not connected)` author lines** (§9.6): in every payload and view an
   agent gets, an unconnected LAN viewer's name reads `<name> (not
   connected)`, and so does the name on unattributed history (comments
   from before migration 12, or from a browser identity never claimed for
   the owner, and batches from before migration 25), the owner's own
   included.
8. **Reserved display names** (§9.6): a LAN viewer cannot take a name that
   looks like the owner's, or a name that looks like a live agent's while
   that agent is live (409 `name_taken`); the owner cannot take a name that
   looks like a live agent's while it is live; and a name with a
   bidirectional embedding, override or isolate control is 400
   `invalid_name`.

§19 covers agent-facing output and existing behaviour. New UI (the People
link and page, the agent strip, owner chips, the Send menu's grouping) is
additive and not listed.

Everything else is byte-for-byte as before: every existing session,
artifact, version, asset and document is the owner's; no person rows
exist, so the filter's fast path applies; LAN viewers keep reaching the
owner's agents through `*unconnected` (O6); the inbox keeps its rows,
`seq`, IDs, keys and read state.

The golden test (plan Tasks 0 and 25) runs on a fixture of main's
migration-21 database holding pre-migration-12 unattributed comments, anonymous and
named LAN viewers, `via_page` comments and documents from the owner's
browser, feedback in every state, and one live agent. It collects every
agent-facing output per tier on a fresh copy of the fixture under a fixed
clock. It normalises two things, each by an explicit list: values minted
during collection or by the test environment (keys `*_at`, `since`,
`waited_s`; ULIDs newer than the fixture; artifact IDs minted during
collection (a create's random ID), as numbered placeholders in
first-seen order, like the ULIDs, wherever they appear (IDs, URLs and
paths); the test server's base URL and
port; the temporary home's path, as in `clip_path`; client PIDs and
working directories, and client executables (the test binary that
answers MCP `status`)), and §19's expected changes (item 6's fields, item 1's
names, item 7's suffix), which it expects rather than ignores. Text
payloads are compared exactly otherwise. The golden is produced on this
branch at Task 0's commit, which adds only an injectable clock (the
store's `now()` and `register_session`'s time) and the collector, no
behaviour change, so it captures main's behaviour at the branch point.
If main moves before this branch merges, Task 0 is rebased onto the new
main and the golden regenerated there, before any later task; that is the
only regeneration allowed. Items 3, 4 and 8 are tested on their own.

## 20. The Toolpath audit journal

The Toolpath audit journal (`2026-10-06-toolpath-audit-design.md`, on main
since migration 23) records every change with `actor` and, for system
actors, `for_actor`, in `audit_events`, in the change's transaction. This
design's callers are recorded as `Caller` counts them (audit spec §5.2,
amended 2026-10-10): a person's agent with its grant and person, the CLI
under a harness and every other request with `X-Clax-Channel: agent` as an
agent, and a connected person's browser as a person. The agent side's
`x-clax-via`, `x-clax-git` and `x-clax-call` count on every verified agent
channel, grants included. `X-Clax-Channel: agent` says whether a request is
an agent's; `x-clax-via` says which agent side sent it; neither replaces
the other.

### 20.1 Actors

| Actor | JSON (audit §5.2 shape) | Toolpath string |
|---|---|---|
| A person's agent | `{"type":"agent", …, "person":"u_…", "grant_id":"g_…"}` (owner agents: `person` absent) | `agent:<harness>/<session>`; `ActorDef.identities` adds `{system:"clax-person", id:"<install>/<public ID>"}` and `{system:"clax-grant", id:<grant ID>}` |
| A grant's agent naming no session | `{"type":"agent","session_id":null,"grant_id":"g_…","person":"u_…","channel":"cli"}` | `agent:clax-grant/<grant ID>` |
| A connected person in a browser | `{"type":"viewer","public_id":"u_…","display_name":"Sam","person":true}` | `human:clax-person/<public ID>` |
| A grant request before approval | `{"type":"client","grant_id":"g_…","client_name":"sam-mbp"}` | `tool:clax-client/<grant ID>` |
| Expiry | `{"type":"system","reason":"expiry"}` | `tool:clax/<version>`, `system_reason` `expiry` |

`via` keeps the channel the agent side names (`mcp`, `hook`, `pi`, `cli`)
for a grant's requests too; `grant_id` in the actor says it came from
another machine, so no `remote` channel is added.

### 20.2 Events

| Kind | Body | Actor |
|---|---|---|
| `invite.create`, `invite.revoke` | `invite_id, person, expires_at` (`rights` on a create that sets them) | owner |
| `grant.request` | `grant_id, flow (invite\|request), client_name, peer_ip, key_sha256, access` | client |
| `grant.approve` | `grant_id, person, access, code_checked` | owner (invite: the client, at once; the body names the person) |
| `grant.deny` | `grant_id, reason (owner\|wrong_code\|expired)` | owner or system |
| `grant.revoke` | `grant_id, by (owner\|person\|expiry), sessions_ended[]` | owner, person or system |
| `grant.token` | `grant_id, op (issue)` (a revoked token ends its grant: `grant.revoke`) | client |
| `grant.narrow` | `grant_id, narrow` | person |
| `person.rights`, `person.revoke` | `person, rights` / `person` | owner |
| `agent.name` | `name, source, previous` | the namer |
| `agent.narrow`, `agent.elevate` | `session, access` | person, owner |
| `approval.request` | `request_id, approver, author, artifact_id, thread_id, cause, count`; recorded when a request is made and when new withheld content raises its `count` | asking agent or router |
| `approval.decide` | `request_id, scope` (`thread`, `artifact`, `everywhere` or `decline`); one per request an approval closes | approver |
| `approval.withdraw` | `request_id, reason (thread_resolved\|thread_deleted\|artifact_deleted\|session_end\|person_revoked\|thread_taken)` | whoever made the change that withdrew it |
| `approval.grant`, `approval.revoke` | `approval_id, approver, author, scope, created_via` (the artifact and thread in the event's IDs) | approver |
| `content.withheld` | `feedback_ids[], author, thread_id, session` | system `daemon`, `for_actor` the agent (once per placeholder) |
| `thread.take` | `thread_id, session, cause (send\|mention\|first_delivery\|reply\|handoff)` | sender, the taking agent, or the agent handing off |
| `thread.release` | `thread_id, session, reason (explicit\|session_end\|grant_revoked)` | agent or system |

`thread.send` gains `approval_request_ids[]`, present when the send made or
joined a request. Reads remain unrecorded.

Recorded since the rebase onto the audit journal (2026-10-10; audit spec
§6.13): `invite.*`, `grant.*`, `person.rights`, `person.revoke` and
`agent.name`; since migration 25, `approval.grant` and `approval.revoke`.
A grant request's flow is `flow`, and where an approval was made
`created_via`, since `via` is the event envelope's channel. The other kinds arrive with the tasks that add their
changes. People, invites and grants need no backfill: they arrive after
the audit migration, so the journal records them from their first row.

## 21. Security

**Threat model.** Other people on the LAN, and the pages and comments they
write, are untrusted input to a person's agents. Clax's job is that its
agent channels hand an agent nothing written by someone its person has not
approved; that agents cannot widen their own rights or approvals; and that
credentials sniffed on the LAN cannot be reused. Clax does not make the LAN
private: pages and threads stay readable by any LAN viewer, responses
cross the LAN in clear (O7), and agents that browse as LAN viewers go
around the filter. **An owner agent with shell access is trusted at the
owner's level:** it runs as the owner's user, so it can read `~/.clax`,
run the CLI by hand, and fetch the owner's browser credential from
`/api/token`; Clax neither can nor tries to stop that. The skills tell
agents not to.

1. **What the filter is (§8.5).** A guarantee for every Clax agent channel,
   field by field, for the owner's and every person's agents alike,
   including whatever a harness runs (slash commands and hooks through
   the wrapper, `!clax …` lines, and the agent's own `clax` calls in its
   shell, detected from the harness's variables, §16); not an
   access-control boundary on a public LAN. An agent that deliberately
   unsets those variables or sets `CLAX_CHANNEL=owner` is going around the
   filter, which the shell-access trust model below covers.
2. **Impersonation.**
   - *Of a person by an agent:* attribution is by session, never by text;
     agent names cannot equal a person's name (§11); the person is shown
     beside an agent's name.
   - *Of a session by another grant:* sessions are bound to their grant in
     registration, join, listing, paths and headers (M4).
   - *Of the owner or an existing person by a new machine:* a request can
     only make a new person (§6.3), so "approve WXYZ-4821, I'm Sam" yields
     a new person named as the owner chose, never Sam's identity or the
     owner's.
   - *Of a person by an unconnected viewer:* unconnected names are marked
     `(not connected)` and cannot equal a connected person's (§9.6).
3. **Confused deputy.**
   - *Page-written content* carries every author of the page, fixed at
     write time (M11).
   - *Approvals and rights* need a browser (M20): no Clax tool, no CLI
     command and no remote agent can approve content, change rights,
     elevate, narrow, invite or approve grants. An owner agent with shell
     access can (it can fetch the owner's browser credential); it is
     trusted at the owner's level. The single exception is M15's
     connect-time answer (§6.3): the connecting person's yes to "Let
     <owner>'s comments and pages reach your agents everywhere?", which the
     grant's continuation token records once, within ten minutes of the
     tokens' issue, and never after the person has made or revoked an
     approval of the owner; `clax connect` refuses to run on an agent
     channel, so no agent answers it for the person.
   - *Carried files and documents* keep their authors (§8.2): carrying a
     file forward or rewriting a document does not move its words to a
     new author.
   - *Oracles:* queries run over visible documents only, and
     content-dependent writes on withheld documents are refused:
     `str_replace` and every merge (`PATCH`, a batch `update`, `acquire`
     with `data`), before anything is evaluated (§7.5).
   - *Agent replies* are recognised from `Caller`, so a grant's reply can
     never be stored as an anonymous (`*unconnected`) comment (M8).
4. **Token theft on the LAN.** Signed requests (M5): a sniffed request
   replays only within its window and never twice; a sniffed token signs
   nothing. Not protected (O7): confidentiality, and a connected person's
   browser cookie, which a sniffer can reuse to act as that person in the
   browser (their inbox, their agents' questions, their approvals). The
   owner's browser credentials are loopback-only. People shows "Use a
   trusted LAN".
5. **Invite races.** An invite code crosses the LAN in clear inside a
   signed request; a sniffer racing it redeems first. Sam then gets
   `invalid_invite`, and the owner sees a connection from an unexpected
   key and peer in the inbox and People, and revokes it.
6. **Replay.** Nonces per key for 240 s against a ±120 s window, checked
   with the signature before every handler; one-time codes are deleted on
   use.
7. **Laundering through agents and pages.** An approved author who
   quotes an unapproved one carries the quote; content an agent writes
   after reading becomes its person's; a page runs in a person's browser,
   which sees everything, so a page can copy text it reads into a new
   comment or document as that person (and its publisher); deleting a
   document and writing a new one at its path starts a new author set;
   an agent that may update an artifact can confirm a withheld file's
   name only by replacing or removing it (`files_withheld` then drops),
   a visible, destructive version credited to its person (a put that
   collides with a withheld path, by letter case or as a file and a
   directory, drops that path as a remove would, so a collision is never
   refused on a withheld name); an artifact's document and lease caps
   (5000 documents, 100 leases in force) count every document and lease,
   and document versions come from one sequence per artifact, so an agent
   can count others' documents, leases and writes, never read them.
   Known limitations.
8. **Denial of service.** The token, timestamp and nonce are checked
   before the body is read; with a sniffed token, unverified bytes in
   flight are bounded per key and overall through bounded queues,
   unverified reads must keep a minimum rate and end by a deadline, and
   bad signatures spend no nonces (§6.6). Known limitation: a sniffed
   access token, without the key, can still hold its victim's large-body
   permit for up to that deadline (10 s plus 1 s per MiB), again and
   again; it can never produce a verified request (§25);
   limits on pending requests, wrong codes, open approval requests,
   nonces, invites and grants (20 live per person).
9. **Names reach other agents unfiltered.** Person names (≤ 60
   characters), agent names (≤ 32), client names (≤ 64) and the document
   paths an agent itself names appear in placeholders, `taken_by` and
   author lines: a small injection surface. Withheld documents' paths are
   never listed (§8.3).
   They are quoted in payloads and shown as text.
10. **Untrusted text** everywhere is rendered as text; the CLI escapes
    control and bidirectional characters.
11. **Logs** never hold tokens, codes, signatures, keys or withheld
    content.
12. No `unsafe`; no hard links.

## 22. Failure modes

| Situation | Behaviour |
|---|---|
| The remote daemon is down | Tools return `daemon_unreachable` naming the remote; hooks exit 0; nothing starts a local daemon while `remote.json` exists. |
| The owner's LAN address changes (DHCP) | `remote.json` names an IP literal (the host rule requires one); requests fail as unreachable; `clax connect --status` and `clax doctor` say so; `clax connect --url <new>` rewrites it, keeping the key and grant. |
| Clock skew beyond 120 s | `stale_request` with the daemon's time; the client retries once with its offset; `--status` and `doctor` report persistent skew. |
| The machine slept | Tokens do not expire (M5); sessions idle over 30 minutes were ended (§14); the agent's next call registers anew, gets its watches back and a note saying what ended (§6.7). A grant unused for 30 days expired: connect again with a new invite. |
| Grant revoked mid-session | 401 `grant_revoked`; tools say "this machine was disconnected from <url>"; its sessions already ended. |
| Approval decided or revoked during `wait_for_feedback` | The generation bump wakes the wait; its next take uses the current approvals (M17). |
| Two agents take an untargeted thread at once | One transaction wins (M23). |
| Owner deletes an artifact with open requests | Requests withdrawn; items read "gone". |
| A person row is folded (claim) | `person`, `author`, `approver`, `recipient` columns follow the fold. |
| `remote.json` exists but the key is unreadable | Every command fails with `remote_key_unreadable` and how to reconnect; no silent fallback. |

## 23. Time to usable and performance

- **Solo installs:** the fast path costs one atomic load per agent request.
  The `/a/…` bootstrap and gallery are unchanged.
- **Baseline first.** `scripts/perf-daemon.py` gains a phase measuring
  `GET /api/artifacts/<aid>/threads` (200 comments) and the feedback poll
  (20 rows) as the owner's agent, recorded on main before the filter
  lands; the budgets below are that baseline plus the stated margin.
- **Budgets** (two connected people, 50 approvals each, 6,000 comments
  across 200 threads with mixed authors):
  - `vis_threads_ms`: baseline + 2 ms, median, as a person's agent.
  - `vis_feedback_poll_ms`: baseline + 1 ms, median, half the rows
    withheld.
  - `sig_verify_us` 150: median signature verification in the gateway.
  - The probe loads under `cheap_p95_ms` and `cheap_max_ms` with a signed
    client polling beside them.
- **Bundles:** the `people` module is lazy (16 KiB); `gallery` and
  `artifact` grow by at most 512 bytes; `extPanel` stays within 65,536.
- **Connecting:** `clax connect` with an invite to "Connected" in under 2 s
  on the LAN; the person's first agent is usable on its first tool call.

## 24. Testing

- **clax-core:** rights; signatures and the nonce cache with a fake clock;
  grant, invite and token state machines; session binding (PID collisions
  across grants); visibility for every field of §8.2 and every scope;
  document author sets; the generation counter; take with field filtering
  and placeholders once; first-take races; names; migrations 24–26 from
  21; the delivery guard.
- **clax-server:** grant flow; `Caller` resolution (every agent channel, including the hooks' path sessions and `/mcp`'s marker); signatures on bodyless requests; session binding on
  register, join, list, paths and headers; the route guard and the
  no-inline-credentials guard; the canary sweep; the visibility matrix;
  approvals and routing; browser-only changes; handoff; cleanup;
  person-scoped inbox, questions, agents.
- **clax-mcp / clax-cli / Pi:** remote mode against a real daemon; `clax
  connect`; clip fetch; new tool arguments; Pi's signing matches Rust's
  test vector.
- **web:** components with hostile text; Playwright end to end with two
  people; screenshots at desktop and phone width.
- **Golden:** §19.
- Fake clocks and injected durations; no fixed sleeps; the gate stays
  within about 2 minutes.

## 25. Known limitations

- No TLS (O7).
- Not an access-control boundary on the LAN (§21).
- Revocation cannot unread; laundering through agents is not tracked.
- Remote Codex sessions have no tier 5; remote agents have no live pages.
- Agent names are unique only among live sessions.
- Approvals are per person, not per agent.
- Names are a small unfiltered injection surface (§21.9).
- A sniffed access token without its key cannot make a verified request,
  but it can hold its victim's large-body permit (and a queue of 4 behind
  it) and up to its route's `LIMIT` of the budget for as long as an
  unverified read may last, 10 s plus 1 s per MiB (§6.6, §21.8), and
  again after each.

## 26. Open questions

- **UI questions for the owner, in one batch before the first UI task**
  (recommendations in brackets): (a) People as its own page [own page] or a
  tab of the inbox; (b) the approval card: four buttons [four] or "Allow ▾"
  with scopes in a menu; (c) the agent strip: chips above the inbox
  [chips] or a left rail on wide screens; (d) withheld content on the
  approver's card: first 3 lines then "Show all" [yes] or collapsed; (e)
  the owner chip on thread cards: name only [name only] or name and person
  always.

## Revision history

- **Revision 1** (2026-10-08): first draft.
- **Revision 2** (2026-10-08), after review and the owner's answers:
  - Recorded the owner's answers (O6–O9) and the rulings (R1–R4); only the
    UI batch stays open.
  - Sessions are bound to their grant on registration, join, listing, paths
    and headers (M4, §6.7).
  - Filtering is per field, in reads, write responses and every tier (M10,
    §8.2–§8.4); write routes that return content are `Filtered`; a
    complete route table (§7.5); assets, labels, file names, icons and
    `resolved_by` have authors.
  - One `Caller` replaces inline token checks (M8); rights are declared on
    routes by a typed extractor instead of a second path matcher (M7);
    authentication happens before the body is read.
  - Approvals, rights, elevation, narrowing, invites and grant decisions
    need a browser (M20); the CLI prints the People link.
  - `*unconnected`'s consequence is stated beside its switch; unconnected
    authors are marked and cannot take a connected person's name.
  - Ad-hoc requests make only new people; machines join existing people and
    the owner by invite.
  - Visibility and rights follow a generation counter at every take (M17).
  - Documents keep author sets; `where`/`order_by` run over visible
    documents only.
  - Tokens do not expire; rotation is on demand with the manage token and
    no grace; grant modification is dropped; departures 9–13 added.
  - The browser link never folds; revoked people lose person routes; remote
    sessions idle 30 minutes; nonce window 240 s and 65,536 per key;
    elevation `until` dropped; approval overflow is a 429 with a People
    banner; a page's `sendToClaude` routes; `/mcp` classified.
  - §19 lists every single-owner change and specifies the golden test; §21
    states the threat model and that the filter is not a LAN boundary;
    budgets start from a measured baseline.
- **Revision 3** (2026-10-08), after the re-review:
  - Every agent channel is an agent (M8): a token request naming a session
    in its path (hooks, `clax feedback follow`) or carrying
    `X-Clax-Channel: agent` (every agent-side client, `/mcp`'s internal
    client included) is `OwnerAgent`; only the CLI used by hand is
    `Owner`. The canary sweep drives the real clients.
  - Signatures are verified before the handler on every request,
    bodyless ones included (§6.6).
  - M20 and §21 state exactly what a browser requirement stops: the Clax
    tools, the CLI and remote agents; an owner agent with shell access is
    trusted at the owner's level. The skills say not to touch approvals.
  - Carried-forward files keep their authors (`versions.file_authors_json`);
    document author sets only grow until deletion; content-dependent writes
    on withheld documents are refused; withheld documents are counted, not
    listed; `capabilities` is author text.
  - §19 lists `(not connected)` lines and the reserved owner name; the
    golden is produced at this branch's Task 0 (an injectable clock and the
    collector, no behaviour change) and normalises minted values by an
    explicit list.
  - Remote CLI commands are listed; M20 routes take `SameOrigin`; M17's
    bumps cover folds and person revocation, with waiters registered before
    takes; re-registration after an idle end restores watches and says
    what ended.
  - Cut: extension approval cards (the panel links to the shell) and token
    rotation (revocation stays).
- **Revision 4** (2026-10-08), after the third review:
  - The plugins' wrapper exports `CLAX_CHANNEL=agent` and the CLI then
    sends the agent-channel marker, so plugin slash commands, hooks and an
    agent's `clax` calls through the plugin are agent channels; `GET
    /api/threads` is Filtered; Denied routes with other people's text
    refuse owner agents; the skills point agents at the MCP tools; the
    sweep runs every slash command's context through the real wrapper,
    and a guard checks every plugin invocation goes through it.
  - Signatures are verified in a `route_layer` keyed by `MatchedPath`
    with each route's `LIMIT` declared beside its right; unverified bytes
    in flight are bounded per key and overall; a nonce is recorded only
    after its signature verifies.
  - The golden normalises the test server's URL, the temporary home, and
    client PIDs and working directories; it is regenerated only on a
    rebased base, before any later task.
  - `GET /api/agents` and `GET /api/approvals*` are Own for a person's
    agents and remote CLI; a session-less remote `comments reply` is
    `no_session`.
  - Sessions record `ended_reason`, and idle ends save their watches for
    the restore.
  - Revoking the access token revokes the whole grant.
- **Revision 5** (2026-10-08), after the revision 4 check:
  - The CLI is an agent channel whenever it runs under a harness:
    `CLAX_CHANNEL=agent` from the wrapper or Pi, or the harness's shell
    variables (`CLAUDECODE`, Codex's and Grok's, confirmed by a probe per
    harness); `CLAX_CHANNEL=owner` is the owner's explicit escape. M8,
    §6.1, §8.5, §16 and §21.1 say what is detected.
  - The `not_for_agents` rule covers every Denied handler that builds a
    view of others' content (including `/api/live/site` and
    `/api/questions*`, now person-scoped), enforced by a guard; the canary
    sweep calls Denied routes and `GET /api/threads` as the owner's agent.
  - Large unverified bodies wait for the key's permit and the budget in
    bounded queues; unverified reads keep a minimum rate (408 below it);
    the client retries `limit_reached` and `busy` with backoff.
  - The wrapper guard follows `tool-hook.sh` and allowlists Pi's two
    direct spawns; `verify_layer`'s pass-through, lookup by method and
    `MatchedPath`, placement and chunked accounting are stated; CLI
    commands refused as `not_for_agents` print a readable line.
- **Amendment** (2026-10-09, plan Task 2):
  - Main's migration 22 (covering indexes) landed first, so these
    migrations are 23–25 (M27, §5, §19, §24). Main's migration 23 (the
    Toolpath audit journal) landed next, so they are 24–26.
  - §5.1 rebuilds `sessions_harness_id` with the credential, so a harness
    session ID is unique per `grant_id` (M4): with the global index, a
    grant's registration with another credential's live harness session ID
    would fail on the index, or block that credential's own registration.
  - §5.1 keeps the browser link's code on its grant (`browser_sha256`,
    `browser_expires_at`), so a link survives a daemon restart (upgrades
    restart the daemon); redeeming it clears it.
- **Amendment** (2026-10-09, plan Task 5, review rulings):
  - §6.1: `Owner` carries `browser`, so the grant decisions (M20) tell
    the owner's browser from the owner's token.
  - §6.6: an unverified read has an absolute deadline (10 s plus 1 s per
    MiB declared), beside the 64 KiB/s floor, and a key's small bodies
    hold at most 8 MiB in flight; a `Filtered` route not yet live answers
    a grant 503 `not_yet` before reading its body.
  - §6.7: the token lists only token sessions; an unknown session ID is
    400 `unknown_session` (a grant) or the handler's answer (the token),
    and 403 `foreign_session` only for another credential's session; a
    path session acts with its own rights; a grant's `pid` is not kept.
  - §25: the sniffed-token hold on a victim's large-body permit.
- **Amendment** (2026-10-09, plan Task 6, review rulings):
  - §16's channel table lists what `scripts/probe-harness-env.sh` saw:
    Pi sets `PI_CODING_AGENT=true`; `GROK_SESSION_ID` is not read until
    Grok's shell is probed.
  - §7.5: `/_blob`, `/c/…` and the artifact's shell page carry the
    artifact's content, so they refuse the owner's agents with
    `not_for_agents` as the other Denied views do.
- **Amendment** (2026-10-10, plan Task 7, owner decisions):
  - §11: a worktree's agent is named after its repository (the common
    git directory), not the worktree's directory; the examples name the
    owner's main-checkout agent `clax-claude`, as `clax` is reserved.
  - §11: a sentence's closing full stops after `@name` are not part of
    the name; chosen names end in a letter or digit.
  - §11, §19 item 8: the owner cannot take a live agent's or a connected
    person's name.
  - §9.6: a new person cannot take the name of a viewer who was never
    invited and is not revoked; the refusal says to invite that viewer.
    A revoked person's or a lapsed invite's name is free again. Display
    names may use any script or mix of scripts; they refuse bidirectional
    embedding, override and isolate controls, and a name that looks like
    the owner's, a connected person's or a live agent's (a shared UTS #39
    skeleton) is `name_taken` (review I1, re-review N1); §19 item 8 lists
    what this changes for a single owner.
- **Amendment** (2026-10-10, rebase review I1):
  - §6.6: the canonical string is `clax-ed25519 v2`, adding a last line,
    the SHA-256 of a fixed serialization of `X-Clax-Session`,
    `X-Clax-Channel`, `x-clax-via`, `x-clax-git` and `x-clax-call` (an
    absent header distinct from an empty one; a repeated one refused).
    `v1` is dropped, not kept as a fallback: no client shipped it.
- **Amendment** (2026-10-10, plan Task 8 review C1, I1, I2, ae90d3cc):
  - M11, §8.2, §21: a page's writes (comments, documents, uploads, its own
    publish, a metadata change) count every author of the page, the
    authors of every file of its current version, not only the current
    publisher: a script carried forward from another person runs in the
    page as much as its `index.html`. A chain of page publishes so keeps
    every earlier author.
  - §5.1: `versions.page_person` is dropped; a `by_page` version's written
    files record the page's authors in `file_authors_json`.
  - §5.2: `comments.page_person` gives way to `page_authors` (a set, in
    `doc_authors`'s shape, for comments and uploads);
    `artifacts.text_authors_json` gives each text field its own authors,
    set by publish and `PATCH`, carried otherwise; `send_batches.sender`
    holds a batch's sender as an author (none: `*unconnected`). The
    `*unconnected` approval is made by the migration's Rust step with a
    ULID, and its `approval.grant` recorded as the store opens.
- **Amendment** (2026-10-10, plan Task 12 review): §8.6's guard and delivery
  text names the filtered takes (`Store::take_feedback_readable`,
  `Store::take_notices`) as standing in for `vis.filter_` on the feedback
  and notices polls.
- **Amendment** (2026-10-11, plan Task 13 and its review):
  - §5.3: the indexes a list or read mark walks are led by the recipient;
    `inbox_by_created`, `inbox_by_thread`, `inbox_by_question` and
    `inbox_by_request` stay keyed by their source (the newest stamp and a
    date range's seq bounds hold for every recipient; a thread move
    updates every recipient's items); `viewers_people` lists the connected
    people. Each question names its person (`questions.person`, indexed
    for each person's open and closed lists), and the open-question cap is
    per person.
  - §18: a person's inbox and question views are not filtered, never name
    a live page, and show `project` only as §13 allows.
- **Amendment** (2026-10-12, plan Task 17 and its review):
  - §7.4: narrowing and elevation are refused for this machine's agents
    (400 `token_agent`), which act with the daemon token that rights do
    not limit (§21 trusts an owner agent with a shell at the owner's
    level); People offers neither for them. The coordinator ruled this
    (record 44f6b8e8). `DELETE /api/agents/<handle>` ends one agent.
  - §9.6 and §19 item 7: unattributed history is marked `(not connected)`
    too, the owner's own included (the owner's decision, 2026-10-12: "Keep
    the mark", record 7fdbaaf5).
