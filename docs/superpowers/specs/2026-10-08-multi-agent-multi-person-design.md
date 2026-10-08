# Multi-agent and multi-person Clax

Date: 2026-10-08
Status: draft for review
Depends on: `2026-09-28-clax-design.md` (the main spec: participants and
attention §10, sessions and the owner identity §11, the MCP surface §12,
security §14), `2026-10-05-chrome-overlay-design.md` (the extension
gateway L5, the owner identity L6, live pages L10) and
`2026-10-06-agent-questions-and-inbox-design.md` (questions, the inbox).
Interacts with: `2026-10-06-toolpath-audit-design.md` (branch
`toolpath-audit`, not yet merged; §20).
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
  path, and a person's agents must not act on content their person has not
  agreed to.

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
  ad-hoc request; each agent then holds a key-bound credential tied to that
  person, revocable per machine and per person.
- An agent has its person's rights. The person narrows them; the owner may
  give one agent more.
- Content written by someone other than an agent's own person never reaches
  that agent, on any read or delivery path, until the agent's person
  approves it (per author per thread, or "always allow" an author on an
  artifact or everywhere). The agent sees a content-free placeholder and can
  ask its person for approval.
- Agents have meaningful names; `@name` reaches exactly one agent.
- A sent thread has one owner agent; others see it taken; handoff is
  explicit; ownership is released when the session ends.
- Ended sessions clean up after themselves, with one inbox note.
- The inbox and the roster are grouped by agent and say which agent has been
  blocked on the person, and for how long.
- A single-owner install upgrades with no change in behaviour.

### Non-goals

- Accounts, passwords, single sign-on, or anything that leaves the LAN.
  Clax stays local; a "person" is a viewer identity of this install.
- TLS. The daemon stays HTTP; §21 says what that costs and what the
  signatures still protect.
- Agent-to-agent messaging, and agents approving content for other agents.
- Hiding content from people. The filter is agent-facing only: a person's
  browser shows what it shows today (the gallery is public on the LAN).
- Live pages for other people. Live pages stay visible only on the owner's
  machine (chrome-overlay L10); other people's agents never see them.
- Removing content an agent has already received. Revocation stops future
  delivery; nothing can be unread.

## 2. Decisions

O1–O5 are the owner's decisions of 2026-10-08 and are binding. M1–M24 are
this design's decisions, each with its reason.

| # | Decision | Reason |
|---|---|---|
| O1 | Connecting a second person's agents: both a one-time invite from People (`clax connect <invite>` on the other machine) and an ad-hoc request (`clax connect http://host:port`, which the owner approves in the inbox). Credentials are tied to the person's identity and revocable per person. | Owner decision. |
| O2 | An agent has its person's rights by default. Its person can narrow the rights of their own agents. The owner can grant one of another person's agents more than that person has. | Owner decision ("not propose only"). |
| O3 | Content from other people reaching an agent is withheld until its person approves: per author per thread, or "always allow <person>" scoped to an artifact or everywhere. The daemon enforces it on every agent-facing read and delivery path: comments, replies, anchor quotes from pages others authored, clips, db documents written by others, artifact files and versions authored by others' agents, all feedback tiers including push and `wait_for_feedback`. Agents see a content-free placeholder and can use `ask` to request approval. Approvals batch per author per thread; revocation stops future delivery. Solo setups are unchanged; today's owner-token agents count as the owner's. | Owner decision. |
| O4 | Also in scope: meaningful agent names (derived from repo, branch and task, agent-settable, person-renameable; `@name` routes to exactly one agent); thread ownership and handoff; agent lifecycle cleanup; agent-aware attention. | Owner decision. |
| O5 | Model permissions and capabilities on GNAP (RFC 9635): grants, access rights, interaction, continuation and token management, and key binding where it fits a local daemon; adapt pragmatically and say where and why the design departs. | Owner decision. |
| M1 | **A person is a viewer.** A connected person is a `viewers` row with `person = 1`; the owner is the owner row. No parallel "principal" table. | Viewers already carry the public ID, the name, authorship, mentions, attention and claiming. A person's browser and their agents share one identity, as the owner's do. |
| M2 | **An agent is a session.** Each agent is a `sessions` row bound to its person (`sessions.person`, NULL for the owner) and, for another machine, to the grant it registered under (`sessions.grant_id`). Rights narrowings and elevations for one agent live on its session and end with it. | Sessions already identify agents (handle, harness, cwd, watches, working records, questions). Agents are short-lived; per-agent state that outlives them would need its own lifecycle. |
| M3 | **One grant per person per machine.** `clax connect` makes one GNAP grant for the person's Clax install on that machine (GNAP's client instance). Every agent session on that machine registers under it and is named in each request by `X-Clax-Session`, checked to belong to the grant. | Agents are many and ephemeral; one key per machine is what a person can manage and revoke. GNAP's model of one client instance per key fits a machine, not a session. |
| M4 | **Tokens are key-bound, never bearer.** `clax connect` makes an Ed25519 key pair; the private key never leaves the machine. Every request carries the access token and a signature over the method, path, query, a timestamp, a nonce and a body digest (§6.6). The owner's daemon token (local, file mode 0600) is unchanged. | The LAN is plain HTTP: a bearer token would be stolen by anyone who can sniff it. A signature with a nonce and a short time window makes a sniffed request useless to replay and a sniffed token useless alone. |
| M5 | **Rights are GNAP access objects** with Clax types (`clax-artifacts`, `clax-comments`, `clax-db`, `clax-ask`), actions, and locations (`*`, `own`, or artifact IDs), plus reference presets (`clax:collaborator`, `clax:reader`, `clax:full`) (§7). An agent's effective rights are `(person ∩ grant narrowing ∩ agent narrowing) ∪ owner elevation`. | O2 in one formula. GNAP's access array is the modelling basis; presets are its reference strings. |
| M6 | **Visibility is separate from rights.** Rights say what an agent may do; visibility (approvals) says whose words may reach it. Approvals are given by the agent's person, never by the owner on another person's behalf. | O3: B's "owner" (B's person) decides what reaches B. The owner of the install decides rights, not another person's consent. |
| M7 | **One filter.** `Visibility` (in `clax-core`) answers "may an agent of person P see content whose effective authors are A₁…Aₙ, located at artifact Y, thread T?" Every agent-facing read and delivery path calls it; a route-guard test pins every route and a store test pins every delivery path (§8.6). | One rule, enforced in one place, checked mechanically, like L10's live-page guard and the audit guard. |
| M8 | **Effective authors.** Content has the set of people whose words it carries: a comment's writer (and, for a comment a page wrote, also the page's publisher); an anchor quote's page publisher; a clip's commenter and page publisher; a version's publisher (and for a page-made version, the previous version's publisher too); a document's writer (and the page's publisher when the page wrote it); a working message's and a batch note's sender; an artifact's title and description, its creator and its current version's publisher. Content is visible only when every effective author is the agent's person or approved by them. | Page-written content is the confused-deputy path: a page running in the owner's browser writes "as the owner". Counting the page's publisher closes it. |
| M9 | **An agent's own person's content is always visible.** The owner's agents see the owner's words; Sam's agents see Sam's. | The filter guards against other people's words, not one's own. |
| M10 | **Approvals are per author, at three scopes**: one thread, one artifact, or everywhere. An author is a person (their agents' words count as theirs). A thread approval covers the author's present and future content in that thread. | O3's wording; one approval covers a conversation, not one message. |
| M11 | **LAN viewers who are not connected people keep today's reach.** Migration 23 gives the owner one approval for the author class `*unconnected` (every viewer who is not the owner and not a connected person) everywhere. The owner can revoke it in People. Connected people are never covered by it. | Zero behaviour change for existing installs (O3: "solo setups unchanged"), while every connected person starts withheld, as O3 asks. Open question Q1 asks whether new installs should start without it. |
| M12 | **Connecting asks the connecting person about the owner.** `clax connect` asks "Let <owner>'s comments and pages reach your agents everywhere? [Y/n]" and records the answer as that person's approval. | A person who connects to someone's Clax is there to work on that person's pages; the default saves an approval on every thread, and declining is one keystroke. |
| M13 | **Withheld feedback stays undelivered.** A feedback row whose content is withheld is not handed over by any tier; instead its session gets one placeholder per (author, thread) (`placeholder_at`), and the row is delivered normally once approved. | Delivery semantics (once, acknowledged, resent) stay exactly as they are for the visible case. |
| M14 | **Routing is an approval request.** A send (button, `to`, `@name`, handoff) that reaches an agent whose person has not approved the thread's authors makes an approval request to that person at once; the agent gets the placeholder and need not ask. | O3's "routing approval": a person sending to someone else's agent is asking that agent's person. |
| M15 | **Approval requests batch per (approver, author, thread).** Later withheld content of the same author in the same thread joins the open request and raises its count. | O3's batching. |
| M16 | **The inbox gains a recipient.** `inbox_items.recipient` is the person it is for (NULL for the owner). Connected people have an inbox; questions from an agent go to its person; approval requests to the approver; connection requests to the owner. | Questions, approvals and attention are per person; the owner's inbox stays exactly as it is. |
| M17 | **Agent names are live-unique tokens.** `[a-z0-9][a-z0-9._-]{0,31}`, unique (ignoring case) among live sessions, never `agent`, `owner`, `clax`, or a person's display name lowercased. Derived as `<repo>` or `<repo>-<branch>` from the session's git directory, with `-<harness>` then `-2`, `-3` on collision; the agent sets it with `status {name}`; its person (or the owner) renames it. A name the agent or person chose that collides is 409 `name_taken`. | `@name` must reach exactly one agent; a token form cannot be confused with a person's mention (whole display name), and reserved words keep `@agent` meaning what it means today. |
| M18 | **The thread's owner agent is its target session.** `threads.target_session_id` (migration 14) becomes the owner: set by a send with `to`, by `@name`, by handoff, and, for a send to everyone, by the first session that takes a delivery of it, which withdraws the other sessions' undelivered rows. Released (cleared) when the owning session ends, its grant is revoked, or the agent releases it. | The existing column already means "where this thread goes"; adding first-take makes "one owner" true for untargeted sends too. |
| M19 | **Handoff is a tool.** `comments_handoff {url_or_id, thread_id, to?}` moves the thread to the named agent (re-sending the thread's viewer comments to it), or releases it without `to`. People hand off by sending again with `to`. | Explicit; one new tool rather than overloading `working` or `comments_resolve`. |
| M20 | **Taken threads refuse other agents' writes.** An agent reply or resolve on a thread owned by another live agent is 409 `thread_taken` naming the owner. Reads stay allowed (filtered). | "Others see it taken": the refusal says who has it, so two agents never answer one thread. |
| M21 | **Session end cleans up.** It withdraws the session's open questions and approval requests it asked, releases its threads, drops its elevations and narrowings, leaves the agent pickers, and makes one `ended` inbox item for its person when it owned threads or had open questions. | O4. One note, only when there was something to clean up. |
| M22 | **Another machine is a remote, not a second daemon.** `clax connect` writes `<home>/remote.json` and the key; while it exists, the shim, hooks, CLI and Pi extension talk to that daemon (signed) instead of starting one. `clax connect --off` removes it. | The agent side already proxies everything over HTTP; one switch in the client keeps every harness working unchanged. |
| M23 | **Remote sessions live by heartbeat only.** The reaper never probes their PIDs (another machine's); a remote session ends after `SESSION_IDLE_SECS` without a heartbeat, or at once when its grant is revoked. Tier 5 by `codex queue` is not available to remote sessions. | The daemon can neither see nor run processes on another machine. |
| M24 | **Migrations 22–24** (people and grants; visibility; per-person inbox), numbered after main's 21. The `toolpath-audit` branch also claims 22: whichever lands on main first keeps 22 and the other renumbers its migrations up before merging; neither edits a shipped migration. | Contiguous migrations; no build of an unmerged branch has run on a real home. |

## 3. User flows

### 3.1 Inviting Sam

1. The owner opens **People** (`/people`), chooses **Invite a person**,
   types "Sam", leaves the rights preset at **Collaborator** (§7.3), and
   presses **Create invite**. People shows
   `clax connect http://192.168.1.5:7480/i/K7QF-M2XD-9PLA` with a copy
   button and "expires in 24 h, one use". (With the daemon bound to
   loopback only, People says to restart it with `clax serve --bind
   0.0.0.0` first; no invite is made.)
2. On Sam's machine, Sam runs that command. It makes a key pair, redeems
   the invite (a grant request carrying the invite), prints the key's
   fingerprint and "Connected to Alex's Clax as Sam", asks "Let Alex's
   comments and pages reach your agents everywhere? [Y/n]", writes
   `~/.clax/remote.json`, and opens Sam's browser at a one-time link that
   makes that browser Sam (§6.5).
3. The owner's inbox gets "Sam connected from sam-mbp (key SHA256:9f…)".
4. From now on Sam's agents (any harness with the Clax plugin) reach the
   owner's daemon. Each registers as Sam's agent with a derived name
   (`clax-multi-person`).

### 3.2 An ad-hoc request

1. Sam runs `clax connect http://192.168.1.5:7480`. The command prints
   "Ask the owner to approve request **WXYZ-4821**" and waits (polling the
   grant's continuation with GNAP's `wait`), up to ten minutes.
2. The owner's inbox shows "Connection request from sam-mbp (192.168.1.40)"
   with **Approve…** and **Deny**. Approve asks for the code Sam sees (the
   owner types `WXYZ-4821`; a mismatch approves nothing), for the person
   (an existing person, an existing LAN viewer, or a new name) and for the
   preset.
3. Sam's command receives its token and continues as in 3.1 step 2.

### 3.3 Withheld content and approval

1. Sam's agent `clax-multi-person` watches the owner's artifact "Quarterly
   Review". Mia (another connected person) comments on it and presses
   **Send to clax-multi-person**.
2. Sam has not approved Mia. The daemon stores the thread and the feedback
   rows for Sam's agent, holds them, and makes an approval request in Sam's
   inbox: "Mia wants to reach your agent clax-multi-person · 1 comment on
   Quarterly Review «Quarterly goals»", showing Mia's comment, with **Allow
   this thread**, **Always allow Mia on this page**, **Always allow Mia
   everywhere** and **Decline**.
3. The agent's next tool result carries, instead of the comment:
   `[clax] 1 comment from Mia on "Quarterly Review" (thread 01J9…) is
   withheld until your person approves it. Your person has been asked.`
4. Sam presses **Allow this thread**. The held rows become deliverable and
   reach the agent by the usual tiers, as any sent comment does.
5. Without routing (Mia comments on a thread the agent already owns), the
   placeholder says `To ask your person, call ask with {"approval":
   {"thread_id": "01J9…"}}`; the agent's `ask` makes the same request and
   waits for the decision.

### 3.4 Narrowing and elevating

- Sam opens People in his browser (his own view: his machines, his agents,
  his approvals) and turns off **Delete artifacts** for all his agents, or
  for one agent. Effective at once.
- The owner, in People → Sam → agents, gives `clax-multi-person` **Publish
  versions of "Quarterly Review"** for this session. That agent may now
  publish there; Sam's other agents may not.

### 3.5 Several agents of one person

- The owner has `clax-main` (Claude Code in `clax`) and
  `clax-multi-person` (Codex in a worktree). The Send button names one of
  them; its menu lists both by name with their state. A comment
  `@clax-multi-person can you take this?` sends the thread to that agent.
- `clax-main` takes a thread; `clax-multi-person`'s `comments_read` shows
  it as `taken by clax-main`; its reply is refused with `thread_taken`.
  `clax-main` calls `comments_handoff` with `to: "clax-multi-person"`.
- The inbox's agent strip reads "clax-main · waiting on you 12 min (1
  question)", "clax-multi-person · working on Quarterly Review".
- `clax-main`'s session ends: its open question is withdrawn, its two
  threads are released, it leaves the Send menu, and the inbox gets one
  note: "clax-main ended · released 2 threads · withdrew 1 question".

## 4. The GNAP model

### 4.1 Roles

| GNAP role (RFC 9635 §1.1) | In Clax |
|---|---|
| Authorization server (AS) | The daemon. |
| Resource server (RS) | The daemon (same process; §4.3 departure 1). |
| Resource owner (RO) | The install's owner: approves grants, sets people's rights, elevates agents. |
| End user | The connecting person (Sam), who runs `clax connect`. |
| Client instance | One person's Clax install on one machine: its key pair and `remote.json` (M3). |
| Subject | The person the grant is bound to (a viewer row). |
| Grant | A `grants` row. |
| Access token | A `grant_tokens` row (hash only), key-bound. |
| Continuation | The grant's continuation token and URI: polling, modification (narrowing), deletion. |

### 4.2 Lifecycle mapped

| GNAP | RFC 9635 | Clax |
|---|---|---|
| Grant request | §2 | `POST /api/grants` with `access_token.access` (rights or a preset reference), `client.key` (`{proof: "clax-ed25519", jwk: {kty: "OKP", crv: "Ed25519", x}}`), `client.display.name`, `interact.start: ["user_code"]`, and for an invite `interact_ref`-like `invite` (departure 3). Signed with the new key. |
| Interaction, user code | §2.5.1.3, §3.3.3 | Ad-hoc: the response's `interact.user_code`; the RO types it when approving (§6.3). |
| Asynchronous authorization | §4.4 | The owner approves later, in the inbox; the client polls continuation. |
| Grant response | §3 | `continue {access_token, uri, wait}`, then `access_token {value, manage: {uri, access_token}, expires_in, access}`, `subject {sub_ids: [{format: "opaque", id: <public ID>}]}`, plus `clax {person: {public_id, display_name}, browser_link}`. |
| Continuation: poll | §5.2 | `POST <continue uri>` with the continuation token, signed; `wait` 5 s; 400 `too_fast` faster. |
| Continuation: modify | §5.3 | `PATCH <continue uri>` with a narrower `access` (the person narrowing every agent of the grant). Only narrowing is accepted from the client; widening is the RO's (§7.4). |
| Continuation: revoke | §5.4 | `DELETE <continue uri>`: the person disconnects the machine (`clax connect --off`). |
| Token rotation | §6.1 | `POST <manage uri>`: a new value, the old one valid for 60 s more. The client rotates when less than a day is left. |
| Token revocation | §6.2 | `DELETE <manage uri>`. |
| Key proofing | §7.3 | `clax-ed25519` (§6.6; departure 4). |
| RO policy, subject | §3.4, §4 | The person's rights (§7) and `subject`. |

### 4.3 Departures, and why

1. **AS and RS are one process.** The daemon both issues and checks tokens,
   so there is no token introspection (RFC 9767) and no RS-first method;
   access objects are read straight from the store.
2. **One grant per machine, agents as sub-identities (M3).** GNAP would
   make each agent session a client instance with its own grant. Agents
   start and end many times a day; a grant per session would put an
   approval in the owner's inbox for every new terminal. Agents are named
   by `X-Clax-Session` inside a grant, and their per-agent narrowing and
   elevation live on the session.
3. **Invites are pre-approved grants.** GNAP has no invite; the RO approving
   ahead of the request is modelled as a single-use reference (`invite`)
   that a grant request carries in place of interaction. Redeeming binds
   the key presented first; a second redemption is 409 `invite_used`.
4. **A narrow key proof instead of HTTP Message Signatures.** `clax-ed25519`
   (§6.6) signs a fixed canonical string with one algorithm, instead of
   RFC 9421's component selection and parameter parsing, which adds a
   parser to every request for flexibility Clax does not need. It keeps
   what §7.3 asks of a proof method: possession of the key, coverage of the
   method, target and body, and freshness.
5. **No finish method, no redirect.** The client is a command line on
   another machine with no callback URI; it polls (`wait`). The browser
   link (§6.5) is not a GNAP interaction but the person's browser joining
   their identity after the grant is approved.
6. **No bearer tokens at all for agents.** GNAP allows `flags: ["bearer"]`;
   Clax refuses it (M4). The daemon token stays a bearer, local only, as
   today.
7. **The RO is not the end user, and visibility is not GNAP.** Approvals of
   content (§10) are the end user's consent to what reaches their agents, an
   information-flow policy the daemon applies as RS; GNAP has no notion of
   it. They live beside grants, not in them (M6).
8. **Access objects carry one non-GNAP field**, `level` on `clax-db`
   (view, interact or admin), because db levels are claude.ai's model and
   predate this design. RFC 9635 §8 allows type-specific fields.

## 5. Data model

All three migrations follow main's 21. Numbers are provisional per M24.

### 5.1 Migration 22: people, grants, sessions

```sql
-- A connected person: a viewer with (or that had) a grant. The owner row
-- is never a person row; the owner's own machines connect as the owner.
ALTER TABLE viewers ADD COLUMN person INTEGER NOT NULL DEFAULT 0;
-- The person's rights (an access array, §7); NULL: the clax:collaborator preset.
ALTER TABLE viewers ADD COLUMN rights_json TEXT;

CREATE TABLE invites (
    id TEXT PRIMARY KEY,                         -- ULID
    code_sha256 TEXT NOT NULL UNIQUE,
    viewer_id TEXT NOT NULL REFERENCES viewers(id),
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
    continue_sha256 TEXT NOT NULL,               -- the continuation token's hash
    narrow_json TEXT,                            -- the person's narrowing of every agent of this grant
    created_at TEXT NOT NULL,
    decided_at TEXT,
    last_used_at TEXT,
    ended_at TEXT,
    ended_by TEXT                                -- 'owner' | 'person' | 'expiry'
);
CREATE UNIQUE INDEX grants_live_key ON grants(client_key) WHERE status IN ('pending', 'approved');
CREATE INDEX grants_by_viewer ON grants(viewer_id, status);
CREATE INDEX grants_pending ON grants(created_at) WHERE status = 'pending';

CREATE TABLE grant_tokens (
    sha256 TEXT PRIMARY KEY,
    grant_id TEXT NOT NULL REFERENCES grants(id),
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    retired_at TEXT                              -- rotated: valid 60 s past this
);
CREATE INDEX grant_tokens_by_grant ON grant_tokens(grant_id);

ALTER TABLE sessions ADD COLUMN grant_id TEXT;         -- NULL: registered with the daemon token
ALTER TABLE sessions ADD COLUMN person TEXT;           -- the person's public ID; NULL: the owner
ALTER TABLE sessions ADD COLUMN name TEXT;
ALTER TABLE sessions ADD COLUMN name_source TEXT CHECK (name_source IN ('derived', 'agent', 'person'));
ALTER TABLE sessions ADD COLUMN narrow_json TEXT;      -- the person's narrowing of this agent
ALTER TABLE sessions ADD COLUMN elevate_json TEXT;     -- the owner's elevation of this agent
CREATE UNIQUE INDEX sessions_live_name ON sessions(name COLLATE NOCASE)
    WHERE ended_at IS NULL AND name IS NOT NULL;
CREATE INDEX sessions_by_grant ON sessions(grant_id) WHERE grant_id IS NOT NULL;

ALTER TABLE artifacts ADD COLUMN person TEXT;          -- the creator's person; NULL: the owner
ALTER TABLE versions ADD COLUMN person TEXT;           -- the publisher's person; NULL: the owner
ALTER TABLE versions ADD COLUMN by_page INTEGER NOT NULL DEFAULT 0;
```

Existing rows get NULLs: every existing session, artifact and version is
the owner's (§19). Existing sessions get no name; names are given at
registration. `person` columns hold public IDs; the viewer claim and fold
(main spec §11) rewrites them with the other author columns.

### 5.2 Migration 23: visibility

```sql
CREATE TABLE approvals (
    id TEXT PRIMARY KEY,                         -- ULID
    approver TEXT,                               -- the person whose agents see; NULL: the owner
    author TEXT NOT NULL,                        -- a public ID, or '*unconnected'
    scope TEXT NOT NULL CHECK (scope IN ('thread', 'artifact', 'everywhere')),
    artifact_id TEXT,                            -- thread and artifact scopes
    thread_id TEXT,                              -- thread scope
    created_at TEXT NOT NULL,
    created_via TEXT NOT NULL CHECK (created_via IN ('shell', 'extension', 'cli', 'connect', 'migration')),
    request_id TEXT,                             -- the request it decided, if any
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
    session_id TEXT,                             -- the agent it is about (the asker, or the routing target)
    cause TEXT NOT NULL CHECK (cause IN ('ask', 'routing')),
    count INTEGER NOT NULL DEFAULT 1,            -- withheld items batched in
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

ALTER TABLE docs ADD COLUMN author TEXT;                 -- the writer's public ID; NULL: the owner (or before this)
ALTER TABLE docs ADD COLUMN via_page INTEGER NOT NULL DEFAULT 0;
ALTER TABLE feedback ADD COLUMN placeholder_at TEXT;     -- a placeholder for this row was handed over

INSERT INTO approvals (id, approver, author, scope, created_at, created_via)
VALUES ('a' || lower(hex(randomblob(12))), NULL, '*unconnected', 'everywhere',
        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'migration');
```

### 5.3 Migration 24: the per-person inbox

`inbox_items.kind` has a `CHECK`, which SQLite cannot alter, so the table is
rebuilt (as migration 19 rebuilt `thread_moves`), keeping every `seq` so
`inbox_fts` rowids still match:

```sql
CREATE TABLE inbox_items_24 (
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
INSERT INTO inbox_items_24 (seq, id, kind, key, artifact_id, thread_id, comment_id, version_n,
    question_id, session_id, harness, detail_json, created_at, read_at)
    SELECT seq, id, kind, key, artifact_id, thread_id, comment_id, version_n,
           question_id, session_id, harness, detail_json, created_at, read_at FROM inbox_items;
DROP TABLE inbox_items;
ALTER TABLE inbox_items_24 RENAME TO inbox_items;
-- every index of migration 21, each led by COALESCE(recipient, ''):
CREATE INDEX inbox_unread ON inbox_items(COALESCE(recipient, ''), seq) WHERE read_at IS NULL;
CREATE INDEX inbox_by_recipient ON inbox_items(COALESCE(recipient, ''), seq);
-- … inbox_by_artifact, _kind, _harness, _session, _created, _thread, _question likewise …
CREATE INDEX inbox_by_request ON inbox_items(request_id) WHERE request_id IS NOT NULL;
```

Keys of items for the owner are unchanged (`reply:<comment_id>`); an item
for another person appends `@<public ID>` (`reply:<comment_id>@u_…`), so one
source makes one item per recipient.

## 6. People, grants and tokens

### 6.1 Who a request speaks for

`Identity` (main spec §11) remains the one place that maps credentials. It
gains one credential kind and one answer:

- **The agent credential** (`Authorization: Clax-Grant <token>` with the
  signature headers, §6.6). The agent gateway (§8.5) verifies it and marks
  the request with `ViaGrant {grant_id, person, session}`. Such a request
  is never the owner (`is_owner()` false) even when the grant's person is
  the owner row: an owner's second machine acts as the owner's agents (its
  sessions have `person` NULL), not as the owner's browser.
- **`Identity::person()`**: `Person::Owner` for any owner credential;
  `Person::Viewer(public_id)` for a viewer cookie naming a person row;
  `Person::Agent {person, session}` for an agent credential; else `None`.
  Every person-scoped route (questions, inbox, approvals, People) reads
  this, so the owner's routes keep `require_owner` semantics for the owner
  and serve a connected person their own items.

The daemon token stays the owner's and needs nothing new. Agents that hold
it count as the owner's (O3).

### 6.2 Invites

- `POST /api/people/invites` (owner) `{name | viewer, rights?}` → `{invite:
  {id, command, expires_at}}`. `name` makes a new person row (`person = 1`,
  no cookie: the browser link binds one later); `viewer` names an existing
  LAN viewer by public ID and marks it a person. The code is 12 characters
  of Crockford base32 in three groups (60 bits), shown once; only its hash
  is stored. Expires in 24 h. 409 `not_on_lan` when the daemon listens on
  loopback only.
- `GET /api/people/invites` (owner), `DELETE /api/people/invites/<id>`
  (owner; revoked).
- The `command` is `clax connect http://<LAN address>:<port>/i/<code>`.

### 6.3 Grant requests

`POST /api/grants` takes a GNAP grant request:

```json
{
  "access_token": {"access": ["clax:collaborator"]},
  "client": {"key": {"proof": "clax-ed25519", "jwk": {"kty": "OKP", "crv": "Ed25519", "x": "<base64url>"}},
             "display": {"name": "sam-mbp"}},
  "interact": {"start": ["user_code"]},
  "invite": "K7QFM2XD9PLA"
}
```

The request is signed with the presented key (§6.6) and carries no token;
the gateway admits exactly this route without one.

- **With `invite`:** a live, unused invite binds the grant at once to its
  person, with the person's rights (`access` may only narrow them): 200
  with `access_token`, `subject` and `clax.browser_link`. An expired,
  used, revoked or unknown code is 400 `invalid_invite` (one message for
  all four).
- **Without:** a pending grant, 201 with `continue {access_token, uri:
  "/api/grants/<gid>/continue", wait: 5}` and `interact.user_code`
  (`WXYZ-4821`: four letters and four digits, shown to the end user). The
  owner's inbox gets a `connection` item. At most 3 pending requests per
  peer IP and 20 in all (429 `limit_reached`); a pending request expires
  after 10 minutes (`expired`).
- The owner decides with `POST /api/grants/<gid>/approve {user_code,
  person: {viewer} | {name}, rights?}` or `POST …/deny`. A wrong
  `user_code` is 400 `wrong_code` and changes nothing; three wrong codes
  deny the request.
- The client polls `POST /api/grants/<gid>/continue` with
  `Authorization: GNAP <continuation token>` (signed). Pending: 200 with
  `continue` again (a poll sooner than `wait` is 400 `too_fast`). Approved:
  200 with the access token as above, and a new continuation token for
  later modification and deletion. Denied or expired: 403 `denied`.

### 6.4 Tokens

- An access token is `cxg_` and 43 base64url characters (32 random bytes);
  only its SHA-256 is stored. It expires 7 days after issue. A grant
  unused for 30 days expires with its tokens.
- `POST /api/grants/<gid>/token` (the manage URI, signed, with the current
  token) rotates: a new token, and the old one is `retired` and valid for
  60 s more. `DELETE` on it revokes the token (the client's
  `clax connect --off` revokes the whole grant through continuation
  instead).
- The daemon keeps an in-memory map of live token hashes to grant (as for
  extension credentials), loaded on start and updated on every change.
- Revoking a grant (the owner in People, the person's `clax connect
  --off`, expiry) ends it and all its tokens at once and ends every live
  session registered under it (§14).
- Revoking a person (owner) revokes their grants and invites; their viewer
  row, authorship and approvals stay as history. Their browser remains an
  ordinary LAN viewer.

### 6.5 The browser link

An approved grant response carries `clax.browser_link`:
`http://<host>:<port>/connect/b/<code>` (32 random bytes, one use, 10
minutes). `clax connect` opens it in the person's default browser. The
daemon answers with a page that sets the `clax_viewer` cookie to the
person's viewer and redirects to `/people`. When the browser already
carried a `clax_viewer` cookie naming a viewer that is neither the owner nor
a person, that viewer is folded into the person (the claim and fold of main
spec §11, unchanged). The link is refused for the owner row (an owner's
second machine keeps its own browser identity) and from a loopback peer
naming a local host (the owner's machine).

### 6.6 Key-bound requests (`clax-ed25519`)

Every request with an agent credential carries:

```
Authorization: Clax-Grant <access token>
Clax-Signature: t=<unix seconds>, n=<nonce: 22 base64url chars>, s=<base64url Ed25519 signature>
```

The signed message is the UTF-8 of

```
clax-ed25519 v1\n<METHOD>\n<path and query as sent>\n<Host>\n<t>\n<n>\n<SHA-256 hex of the body>\n<SHA-256 hex of the access token>
```

The grant request and continuation calls sign the same way with the
continuation token (or nothing, for the first request) in the last line.
The daemon verifies with the grant's `client_key`, accepts `t` within ±120 s
of its clock, and refuses a nonce seen for that key within the last 300 s
(a per-key set bounded at 4,096 entries, evicting oldest; beyond, the
request is 429 `limit_reached`). Errors: 401 `invalid_signature`,
`stale_request`, `replayed_request`, `invalid_token`, `token_expired`;
the client re-signs and retries once on `stale_request` after reading the
daemon's `Date`. A request with an agent credential and an `Origin` or a
`Cookie` header is 403 `forbidden_origin` (a browser cannot hold the key,
and nothing is mixed).

Keys: Ed25519 with `ring` (already in the lockfile, now a direct
dependency of `clax-core`), PKCS#8 in `<home>/remote.key` (0600). Signing
costs about 20 µs and verifying about 50 µs.

## 7. Rights

### 7.1 Access objects

```json
{"type": "clax-artifacts", "actions": ["read", "create", "update", "delete"], "locations": ["*"]}
{"type": "clax-comments",  "actions": ["read", "reply", "resolve", "watch"], "locations": ["*"]}
{"type": "clax-db",        "actions": ["read", "write"], "level": "interact", "locations": ["*"]}
{"type": "clax-ask",       "actions": ["ask"]}
```

| Type | Action | Routes it opens |
|---|---|---|
| `clax-artifacts` | `read` | list, get, versions, files, assets list (filtered, §8) |
| | `create` | `POST /api/artifacts` (location ignored) |
| | `update` | publish a version, patch metadata, pin, upload assets |
| | `delete` | delete an artifact, delete an asset |
| `clax-comments` | `read` | threads list and get, clips (filtered) |
| | `reply`, `resolve` | agent replies and resolves |
| | `watch` | watch, `wait_for_feedback`, the feedback poll, notices, working records |
| `clax-db` | `read`, `write` | the doc routes; `level` caps the caller level (`view` < `interact` < `admin`) |
| `clax-ask` | `ask` | the session's question routes and approval requests |

Locations: `*` (every artifact), `own` (artifacts the agent's person
created: `artifacts.person`), or artifact IDs. Everything not in this
vocabulary (live pages, `sample`, the extension, sessions of others,
shutdown, the token, other people's inboxes, People, grants) is never
grantable: only owner credentials reach it, and the agent gateway refuses
it (§8.5).

### 7.2 Effective rights

```
effective(agent) = (person_rights ∩ grant.narrow ∩ session.narrow) ∪ session.elevate
```

- `person_rights` is the owner row's `clax:full` for the owner's agents
  (unchanged: they hold the token anyway) and `viewers.rights_json` (or
  `clax:collaborator`) for a person.
- `∩` keeps an action at a location only when both sides allow it; `level`
  takes the lower. A missing narrowing is no narrowing.
- `∪` adds the owner's elevation, which may exceed `person_rights`.
- `own` is resolved at check time (an artifact created by the person's
  agents is `own`, whichever agent created it).

A pure `Rights` type in `clax-core::rights` implements parse, presets,
`intersect`, `union`, `allows(type, action, artifact, person_of)` and a
`level_for(artifact)`.

### 7.3 Presets

| Preset | Rights |
|---|---|
| `clax:reader` | artifacts `read` `*`; comments `read`, `reply`, `watch` `*`; db `read` `*`; ask |
| `clax:collaborator` (default for a person) | reader, plus artifacts `create`, `update` `own`, `delete` `own`; comments `resolve` `*`; db `write` `*` at `interact`, and at `admin` on `own` |
| `clax:full` | every action at `*`, db `admin` |

### 7.4 Who changes what

- The owner sets a person's rights (People → person → Rights, or
  `PUT /api/people/<public_id>/rights`), and elevates one agent
  (`PUT /api/agents/<handle>/elevate {access, until?}`; `until` defaults
  to the session's end, at most 30 days). Removing an elevation is
  `DELETE`.
- A person narrows all their agents of one machine
  (`PATCH /api/grants/<gid>/continue` from that machine, or
  `PUT /api/people/me/grants/<gid>/narrow` from their browser) and one agent
  (`PUT /api/agents/<handle>/narrow`). A narrowing that would widen is 400
  `not_narrower`.
- An agent cannot change its own rights.
- A change applies to the next request; nothing in flight is cut.

## 8. The visibility filter

### 8.1 The question it answers

"May an agent of person P see content C, whose effective authors are
A₁…Aₙ, at artifact Y (thread T)?" Yes when, for every Aᵢ:

1. Aᵢ is P (M9); or
2. Aᵢ is a system author (Clax itself: placeholders, notes the daemon
   writes); or
3. P has a live approval for author Aᵢ (or, when Aᵢ is not the owner and
   not a connected person, for `*unconnected`) at scope `everywhere`, or
   `artifact` with Y, or `thread` with T (when C is in a thread).

Otherwise C is withheld from that agent.

### 8.2 Effective authors

| Content | Effective authors |
|---|---|
| A viewer comment | its writer (`author_public_id`; anonymous counts as `*unconnected`) |
| A comment the page wrote (`via_page`) | its writer and the publisher of the version the thread is on |
| An agent comment (reply) | the person of its session |
| A thread's anchor (quote, prefix, suffix) | the publisher of the thread's version |
| A clip | the thread's first commenter and the publisher of the thread's version |
| A version's files and note | its publisher (`versions.person`); for a `by_page` version, also the previous version's publisher |
| An artifact's title and description | its creator (`artifacts.person`) and its current version's publisher |
| A document | its writer (`docs.author`); with `via_page`, also the current version's publisher |
| A working record's message | its session's person |
| A batch note | its sender |
| A question's text and answers | the asking agent's person (always P: questions reach only their own session) |

"The owner" is NULL in every `person` column and the owner's public ID in
`author_public_id`; the filter normalises both to `Owner`. A live page's
snapshots and anchors (owner-only, L10) count as the owner's.

### 8.3 Placeholders

Withheld content is replaced, never dropped, so the agent knows something
is there and whom to ask:

- **A comment**: `{"id", "author": {"kind", "name", "person"}, "withheld":
  true, "body": null, "created_at"}`.
- **A thread**: its view gains `"withheld": {"count", "authors": [{"public_id",
  "name"}], "approval": "none" | "requested" | "declined"}`; a withheld
  anchor has `quote`, `prefix`, `suffix` null and `"withheld": true`; a
  withheld clip has no `clip` and `"clip_withheld": true`.
- **An artifact** (list, read): `title` and `description` null,
  `"withheld": {"authors": […]}`; its files and versions answer 403
  `withheld` with the same object.
- **A document**: `{"doc_id", "version", "withheld": {"authors": […]}}`, no
  `data`.
- **A working message, a version note, a batch note**: null with
  `"withheld": true`.
- **Feedback text** (every tier): one line per (author, thread):
  `[clax] N comments from <name> on "<title or "an artifact">" (thread
  <ID>) are withheld until your person approves them.` followed by `Your
  person has been asked.` when a request is open, or `To ask your person,
  call ask with {"approval": {"thread_id": "<ID>"}}.` otherwise. Names are
  quoted with `feedback::quoted`. No comment text, quote, clip or title of
  withheld content appears.

### 8.4 Delivery

- `take_feedback` (every tier: piggyback, stop hook, prompt hook, wait,
  inject, queue) partitions the session's eligible rows into visible and
  withheld. Visible rows are delivered as today. Withheld rows are left
  undelivered; those without `placeholder_at` produce the placeholder lines
  and get `placeholder_at` set, so each placeholder is handed over once.
- When an approval makes rows visible, they are deliverable at once and
  the session's waiters are woken (as a send wakes them).
- Notices (Grok's monitor, Claude Code's channel) announce only visible
  rows; placeholders ride the in-band tiers.
- `codex queue` sends only visible content.
- Resends (main spec §10) apply to visible rows only.
- A thread's `feedback_state` shown to people says `withheld` (with the
  approver) for rows held back, so the sender sees why the agent has not
  answered.

### 8.5 Where it is applied: the agent gateway

Agent credentials enter through one middleware, the **agent gateway**,
which sits beside the extension gateway:

1. Verifies the credential and signature (§6.6), resolves the session from
   `X-Clax-Session` (which must be live and belong to the grant, except on
   `POST /api/sessions`), and loads the effective rights.
2. Admits the request only when its method and path are in the gateway's
   allowlist, with the right that entry names (§7.1) at the artifact the
   path names; else 403 `forbidden` (outside the vocabulary) or 403
   `not_permitted` (a right missing), naming the right.
3. Strips `Authorization` and `Clax-Signature`, and inserts `ViaGrant`.

Handlers that today take `RequireToken` for agent work take `RequireAgent`
instead: the token, or a `ViaGrant`. Each derives the reader:
`Reader::Owner` for the token (and any owner credential) or
`Reader::Person(public_id)` for a grant, and builds `Visibility` for it.
LAN viewers' and owner browsers' reads build no `Visibility`: the filter is
agent-facing (owner agents with the token are agents too: a token request
from no browser builds `Visibility` for the owner).

### 8.6 Guard tests

- **Route guard** (`routes/mod.rs`, `mod agents`, beside `mod l10`): every
  route is listed with one class: `Denied` (not in the agent gateway's
  allowlist), `Right(type, action, Filtered)` (the handler builds and uses
  `Visibility`: its source contains `Visibility::for_reader` and a
  `vis.` call), `Right(type, action, NoContent)` (it returns no content
  another person wrote: a write's result echoes the caller's own input),
  or `GrantFlow` (the grant routes). The test reads the routes from
  source, fails on an unlisted route, and checks the gateway's allowlist
  equals the non-`Denied` entries exactly.
- **Delivery guard** (`clax-core`): `FeedbackItem` and `Notice` are built
  only inside `store/feedback.rs` functions that take `&Visibility`; a test
  scans the crate's source for their constructors and fails elsewhere. A
  table test drives every tier with a withheld row and asserts no body,
  quote, clip path or title reaches the result.
- **Behaviour matrix** (`clax-server/tests/api_visibility.rs`): for each
  content kind of §8.2 and each agent-facing route, an agent of a person
  who has not approved the author gets the placeholder, and after a
  `thread`, `artifact` or `everywhere` approval gets the content; after
  revocation, gets the placeholder again.

### 8.7 Cost

`Visibility::for_reader` reads the reader's live approvals once per request
(one indexed query, typically under ten rows) into hash sets; each check is
a few hash lookups. The session-to-person map is cached in `AppState` and
updated on registration and end. **Fast path:** while the install has no
person rows and the `*unconnected` approval is live, `Visibility` for the
owner is `AllVisible` and costs one atomic load, so a single-owner install
pays nothing (§23).

## 9. Approvals

### 9.1 Objects

An approval (`approvals`, §5.2) says "agents of `approver` may see
`author`'s content at `scope`". A request (`approval_requests`) asks the
approver for one.

### 9.2 Requests

- **Routing (M14):** a send, `@name`, or handoff that targets an agent
  whose person has not approved some author of the thread's current
  content makes (or joins) a request per such author: `cause: routing`,
  `session_id` the target.
- **Asking:** `ask` with `approval: {url_or_id?, thread_id?, author?}`
  makes (or joins) a request for each author withheld from the session at
  that thread or artifact, `cause: ask`, and waits for the decision as
  `ask` waits for answers (same timeout and `call_again`). It returns
  `{"approval_id", "status": "approved" | "declined" | "open", "scope"}`.
  Nothing withheld there: 400 `nothing_withheld`.
- **Batching (M15):** at most one open request per (approver, author,
  artifact, thread); new withheld content of that author there raises
  `count` and `updated_at` and re-announces the item (one inbox item per
  request, not per content).
- **Limits:** 100 open requests per approver; beyond, new withheld content
  is held without a request, and the approver's inbox shows one overflow
  note per author ("Mia has more waiting: review in People").
- An open request is withdrawn when its thread is resolved or deleted,
  its artifact deleted, or (for `cause: ask`) the asking session ends
  without other withheld rows there.

### 9.3 Deciding

`POST /api/approvals/requests/<id>/decide {scope: "thread" | "artifact" |
"everywhere" | "decline"}` from the approver (`Identity::person()`; 404 for
anyone else). Approving makes the approval (or finds the live one), marks
the request `approved`, closes every other open request of that approver
and author that the new scope covers, releases the held rows they cover,
and wakes the affected sessions. Declining marks it `declined`; the rows
stay held; the agent's placeholder says `declined`; a later `ask` makes a
new request only after 10 minutes (400 `recently_declined` before).

Approvals made directly (People → person → "Always allow", `clax approvals
allow`) need no request.

### 9.4 Revoking

`DELETE /api/approvals/<id>` (the approver). From the next read or
delivery on, content the approval covered is withheld again unless another
live approval covers it: undelivered rows are held again (their
`placeholder_at` is cleared so the agent is told once), reads show
placeholders. What was delivered stays delivered; the agent may still hold
it in its context. Revocation never ends a session or releases a thread.

### 9.5 Where people decide

- **Inbox** (`approval` items, unread until decided): the card shows the
  author, the agent, the page and thread, the withheld content itself (the
  approver may read it: people are not filtered), the count, and the four
  buttons. Opening the thread shows the same card above it.
- **People** (`/people`): per person, "Their content reaching your agents"
  (live approvals, with Revoke) and "Waiting" (open requests).
- **The extension's panel** (owner only): approval items in the Inbox tab,
  and the card above a live page's thread when one is open.
- **CLI:** `clax approvals [--open]`, `clax approvals allow <person>
  [--artifact A | --thread T | --everywhere]`, `clax approvals revoke <id>`,
  `clax approvals decide <id> <scope|decline>`.

## 10. Routing to another person's agent

- The Send menu lists every live agent that may receive the thread: "Your
  agents", then each person's ("Sam's agents"). An agent of another
  person whose person has not approved this thread's authors is listed
  with "Sam approves first".
- Sending to it (button, `to`, `@name`) stores the send as today and makes
  the routing request (§9.2). The thread's delivery state reads "waiting
  for Sam's approval".
- An agent may only receive a thread on an artifact it may read
  (`clax-comments read` at Y): otherwise the send is 400
  `agent_cannot_read`.
- A page (`comments` capability) cannot route: its `@name` sends nothing,
  as its `@agent` sends nothing today.

## 11. Agent names

- **Derivation.** On registration the agent side sends `repo` (the
  basename of the nearest directory above `cwd` holding `.git`, a
  directory or a worktree's file) and `branch` (read from that
  repository's `HEAD`; detached is none), with no `git` process. The
  daemon names the session `<repo>` when the branch is none, `main`,
  `master` or the repository's only branch it can see, else
  `<repo>-<last path segment of the branch>`; lowercased, every other
  character than `[a-z0-9._-]` made `-`, runs of `-` collapsed, cut to 32.
  Taken: `<base>-<harness>`, then `<base>-2`, `-3`, …. No repository: the
  harness (`claude`, then `claude-2`…).
- **Task.** The agent sets a name for its task with `status {"name":
  "fix-chart-axis"}` (M17 rules; 409 `name_taken`; 400 `invalid_name`), and
  may do so again unless its person renamed it (`name_source = 'person'`:
  409 `name_locked`).
- **Rename.** The person of the agent (and the owner, for any agent)
  renames it in the roster or People, or `clax agents rename <name|handle>
  <new>`; `PUT /api/agents/<handle> {name}`.
- **Collisions.** Names are unique among live sessions (the index of
  §5.1); an ended session's name is free. Reserved: `agent`, `owner`,
  `clax`, `me`, `everyone`, and any person's display name lowercased with
  spaces as `-`.
- **`@name`.** In a comment, `@` followed by a live agent's name as a whole
  token (preceded by start or a non-alphanumeric, followed by the end,
  whitespace or punctuation other than `.`, `_`, `-`) sends the thread to
  that agent: a send with `to`. When the agent is not the artifact's owner
  or a watcher, the send also gives it a direct watch on the artifact. A
  name that names no live agent sends nothing; the comment's response
  carries `unresolved_mentions: ["name"]` and the shell says so. `@agent`
  keeps its meaning.
- **Display.** Agents show as their name, with their person ("clax-main ·
  Sam") wherever more than one person's agents appear, and with their
  harness icon. The payload's author line for a reply is
  `<name> (<person>'s agent, via <harness>)`.

## 12. Thread ownership and handoff

- **Owner.** `threads.target_session_id` is the owner agent (M18). It is
  set by a send with `to`, `@name`, a handoff, or, for an untargeted send,
  by the first `take_feedback` that delivers one of the thread's rows to a
  session: in that transaction the session becomes the owner and every
  other session's undelivered rows for the thread are deleted (they were
  copies of the same comments). A placeholder take does not make an owner.
- **Views.** Thread views (shell and agent) carry `owner: {handle, name,
  person, live}`. An agent reading a thread owned by another live agent
  sees `"taken_by": "<name>"`.
- **Refusals (M20).** `comments_reply` and `comments_resolve` on a thread
  whose owner is another live session: 409 `thread_taken` with the owner's
  name; the result's `note` says to ask the person or wait for a handoff.
  Threads with no owner, or an ended owner, accept any agent with the
  rights (and the first such reply makes it the owner).
- **Handoff.** `comments_handoff {url_or_id, thread_id, to}` by the owner
  agent (or any agent of the owner's person when the owner has ended):
  `to` must name a live agent that may read the artifact; the thread is
  sent to it with `to` (§10 rules, including the routing request for
  another person's agent); the result is `{thread_id, owner}`. Without
  `to`, the thread is released: owner cleared, its undelivered rows
  untargeted (main spec §10's untargeted rules). A non-owner calling it is
  409 `thread_taken`.
- **People** take over by sending again (the Send menu), as today.
- **Release on end.** §14.

## 13. Agent-aware attention

- **`GET /api/agents`** (person-scoped: the owner sees every agent, a
  person their own) → `{agents: [{handle, name, person: {public_id, name}
  | null, harness, project, live, state, since, blocked: {since, questions,
  approvals} | null, threads_owned, working: [{artifact_id, title,
  message}], unread}]}`. `state` is `waiting` (an open question or an open
  `cause: ask` request of the agent's), `working` (a working record),
  `idle` (live, neither) or `ended` (only within the last hour). `blocked.since`
  is the oldest open question's or request's `created_at`. `project` is
  shown only to the agent's person and the owner.
- **The `agents` stream topic** (person-scoped) announces changes.
- **Inbox grouping.** `/inbox` opens with an agent strip: one chip per
  agent with unread items or a block, ordered blocked-longest first:
  "clax-main · waiting on you 12 min". A chip filters the inbox to that
  agent (`agent=<handle>`, existing). `GET /api/inbox?group=agent` returns
  the strip's data with the page.
- **Roster.** The gallery's working roster and the artifact view's agent
  list show the same states and the block time; `clax status` prints them.
- **Notifications** (questions spec §9.7) title an approval request "Mia
  wants to reach clax-main" and a block over 10 minutes is not notified
  again (no new events, no new notification).

## 14. Lifecycle cleanup

When a session ends (its end hook, the shim's exit, the reaper, or its
grant's revocation), in the end's transaction:

1. Open questions are withdrawn (as today).
2. Open approval requests with `cause: ask` and this `session_id` are
   withdrawn; `cause: routing` requests stay (the content waits for the
   person, whoever gets the thread next).
3. Threads it owns are released: `target_session_id` cleared, undelivered
   rows untargeted (main spec §10).
4. `narrow_json` and `elevate_json` are cleared; the name is freed (the
   unique index covers live sessions only).
5. Watches end (as today); working records end (as today).
6. When it owned threads or had open questions or `ask` requests, one
   `ended` inbox item for its person: `detail_json {name, threads:
   [{id, artifact_id}], questions: n, approvals: n}`, rendered "clax-main
   ended · released 2 threads · withdrew 1 question".
7. Pickers and `GET /api/agents` drop it (after an hour in `ended` state,
   for the strip only); the main spec's "agents on an artifact" list keeps
   ended agents only as authors of what they wrote, never in Send menus.

## 15. Contract summary (new and changed routes)

Token or grant (`RequireAgent`), checked by the agent gateway's rights:

| Route | Change |
|---|---|
| `POST /api/sessions` | with a grant: registers under it (`person` and `grant_id` from the grant); body gains `repo`, `branch`; response gains `name`, `person` |
| `PATCH /api/sessions/<sid>` | body gains `name` (the agent's own) |
| existing agent routes | accept a grant; the rights of §7.1; filtered per §8 |
| `POST /api/sessions/<sid>/threads/<tid>/handoff` | `{to?}` → `{thread}` |
| `POST /api/sessions/<sid>/approvals` | `{artifact_id, thread_id?, author?}` → `{requests}`; `GET …/approvals/<rid>?wait=` long-poll |

Grant flow (signed, no token): `POST /api/grants`, `POST|PATCH|DELETE
/api/grants/<gid>/continue`, `POST|DELETE /api/grants/<gid>/token`.

Owner: `POST|GET /api/people/invites`, `DELETE /api/people/invites/<id>`,
`GET /api/grants?status=`, `POST /api/grants/<gid>/approve|deny`,
`DELETE /api/grants/<gid>` (revoke), `GET /api/people`, `PUT
/api/people/<pid>/rights`, `DELETE /api/people/<pid>` (revoke the person),
`PUT|DELETE /api/agents/<handle>/elevate`.

Person (owner or connected person, own items only): `GET /api/people/me`,
`PUT /api/people/me/grants/<gid>/narrow`, `PUT
/api/agents/<handle>/narrow`, `PUT /api/agents/<handle>` (rename), `GET
/api/agents`, `GET /api/approvals`, `DELETE /api/approvals/<id>`, `POST
/api/approvals` (direct), `GET /api/approvals/requests`, `POST
/api/approvals/requests/<id>/decide`; the question and inbox routes,
scoped to the caller's person; the `agents`, `questions`, `inbox` and
`people` stream topics, scoped likewise.

Browser: `GET /connect/b/<code>`; shell page `/people`.

The extension gateway admits the person routes for the owner (the
approval cards).

## 16. MCP, CLI and plugins

- **Tools.** `status` gains `name` (set this agent's name) and reports
  `name`, `person`, `rights` (effective, as access objects) and `remote`
  (the daemon URL, when remote). `ask` gains `approval` and `approval_id`
  (§9.2). New `comments_handoff` (§12). Every tool's errors gain
  `not_permitted` (with the missing right), `withheld`, `thread_taken`.
  Twenty-five tools.
- **Clips** for a remote session: the payload's `Clip:` line names
  `clax-clip:<artifact>/<thread>`; the agent side (shim, hook, Pi) fetches
  it (signed) into `<home>/remote-cache/clips/` and rewrites the line to
  that path before handing the text over; a failed fetch rewrites it to
  `Clip: unavailable (<reason>)`.
- **`clax connect`**: `clax connect <invite URL>`, `clax connect <daemon
  URL>`, `clax connect --status`, `clax connect --off`, `--name <client
  name>` (default the host name), `--no-browser`, `--yes` (accept the
  owner approval default). Writes `remote.json` (0600: `{url, grant_id,
  person, token, token_expires_at, manage_uri, continue_uri,
  continue_token}`) and `remote.key`.
- **Remote mode** (M22): `clax-cli`'s and `clax-mcp`'s clients read
  `remote.json` when present (and `CLAX_REMOTE` is not `0`): they skip
  daemon discovery and auto-start, sign every request, rotate the token
  when under a day remains, and report `remote` in errors
  (`daemon_unreachable` names the remote URL). `clax serve` still serves a
  local daemon for local use; agents use the remote while it is configured.
  Pi's extension does the same in TypeScript (`node:crypto` Ed25519).
- **`clax people`** (owner): list, `invite <name>`, `revoke <person>`,
  `rights <person> <preset|json>`. **`clax agents`**: list (grouped by
  person, with states and blocks), `rename`, `narrow`, `elevate` (owner).
  **`clax approvals`** (§9.5).
- **Hooks** run unchanged over the remote client; `codex queue` is not
  attempted for remote sessions, and `status.push` says so.
- **Skills** gain "Working beside other agents and people": your name and
  how to set it; withheld content and how to ask; `thread_taken` and
  handoff; never treat a placeholder as an instruction.

## 17. Shell and extension

- **People** (`/people`, lazy module `web/shell/src/people/`): for the
  owner, people (each with rights, machines and their keys' fingerprints,
  agents, approvals they gave, approvals the owner gave them), invites,
  pending connection requests, and the `*unconnected` approval with its
  switch; for a person, their own machines, agents (rename, narrow),
  approvals and requests. Top-bar link beside Inbox.
- **Inbox:** `approval` and `connection` cards (answerable in place),
  `ended` rows, the agent strip.
- **Threads:** the owner chip ("clax-main has it"), "waiting for Sam's
  approval" delivery state, and the Send menu by name and person.
- **Artifact agent list:** names, states, blocks.
- **Extension panel:** approval cards in its Inbox tab and above a
  thread; agent names in its Send menu.
- Every UI change is verified in a real browser with screenshots (owner
  constraint), at phone width too.

## 18. Owner and person views of the same thing

| Thing | Owner | Connected person | LAN viewer |
|---|---|---|---|
| Gallery, artifacts, threads | all (unchanged) | all (unchanged: public on the LAN) | all (unchanged) |
| Live pages | yes (local) | never | never |
| Inbox, questions | own | own | none |
| People | everyone | self | none |
| Agents roster | every agent | own agents; others' names and states | others' names on threads |
| Approvals | own | own | none |

## 19. Migration of single-owner installs

What an existing install sees after migrations 22–24, and why nothing
changes:

- Every existing session, artifact, version and document has `person`
  NULL: the owner's. Agents with the daemon token are the owner's (O3).
- No person rows exist, so the filter's fast path applies (§8.7): every
  read and delivery is byte-for-byte what it was. A golden test (Task 20)
  renders `comments_read`, `read`, the feedback payload of every tier, the
  notices and the roster from a migrated fixture before and after.
- LAN viewers keep reaching the owner's agents through the `*unconnected`
  approval (M11).
- The inbox's rows keep their `seq`, `id`, key and read state; the
  owner's routes answer as before.
- New sessions get derived names, which appear in the shell where it
  showed the harness ("Send to clax-main" instead of "Send to claude").
  This is the one visible change, and it is O4's.
- With several of the owner's agents watching one artifact, an untargeted
  send (no live target chosen, `@agent` on a never-sent thread, a page's
  `sendToClaude`) now goes to the first agent that takes it instead of all
  of them (M18, O4). With one agent nothing differs.

## 20. The Toolpath audit branch

The audit branch records every change with `actor` and, for system actors,
`for_actor`, in `audit_events`, written in the change's transaction. This
design's changes are recorded the same way once the branch merges; until
then, each store function that makes one of these changes returns what the
event needs, so the merge adds the record call and nothing else.

### 20.1 Actors

| Actor | JSON (audit §5.2 shape) | Toolpath string |
|---|---|---|
| A person's agent | `{"type":"agent", …, "person":"u_…", "grant_id":"g_…"}` (the owner's agents: `person` absent) | unchanged `agent:<harness>/<session>`; `ActorDef.identities` adds `{system:"clax-person", id:"<install>/<public ID>"}` and `{system:"clax-grant", id:<grant ID>}` |
| A connected person in a browser | `{"type":"viewer","public_id":"u_…","display_name":"Sam","person":true}` | `human:clax-person/<public ID>` |
| A grant request before approval | `{"type":"client","grant_id":"g_…","client_name":"sam-mbp"}` | `tool:clax-client/<grant ID>` |
| Expiry | `{"type":"system","reason":"expiry"}` | `tool:clax/<version>` with `system_reason` `expiry` |

`via` gains `remote` (a request with an agent credential).

### 20.2 Events

| Kind | Body | Recorded at |
|---|---|---|
| `invite.create` | `invite_id, person, expires_at` | owner |
| `invite.revoke` | `invite_id` | owner |
| `grant.request` | `grant_id, via (invite\|request), client_name, peer_ip, key_sha256, access` | the client |
| `grant.approve` | `grant_id, person, access, code_checked` | owner (invite: the redeeming client, `for_actor` the person) |
| `grant.deny` | `grant_id, reason (owner\|wrong_code\|expired)` | owner or system |
| `grant.modify` | `grant_id, narrow` | the person |
| `grant.revoke` | `grant_id, by (owner\|person\|expiry), sessions_ended[]` | |
| `grant.token` | `grant_id, op (issue\|rotate\|revoke), expires_at` (no token) | the client |
| `person.rights` | `person, rights` | owner |
| `agent.name` | `name, source (derived\|agent\|person), previous` | the namer |
| `agent.narrow`, `agent.elevate` | `session, access, until?` | person, owner |
| `approval.request` | `request_id, approver, author, artifact_id, thread_id, cause, count` (on create and on batching) | the asking agent, or the router |
| `approval.decide` | `request_id, scope (thread\|artifact\|everywhere\|decline)` | the approver |
| `approval.grant` | `approval_id, approver, author, scope, artifact_id, thread_id, via` | the approver |
| `approval.revoke` | `approval_id` | the approver |
| `content.withheld` | `feedback_ids[], author, thread_id, session` | system `daemon`, `for_actor` the agent (once per placeholder) |
| `thread.take` | `thread_id, session, cause (send\|mention\|first_delivery\|handoff)` | the sender, or system for first delivery |
| `thread.release` | `thread_id, session, reason (explicit\|session_end\|grant_revoked)` | |

`thread.send` (audit §6.2) gains `approval_request_ids[]` when routing made
requests. Reads remain unrecorded (audit §6.11), withheld or not; the
placeholder hand-over is recorded because it is a delivery.

## 21. Security

What is protected, from whom:

1. **Impersonation.**
   - *Of a person by an agent:* an agent's words are attributed by the
     daemon to its session's person, never by text; author lines are
     built by the daemon; names cannot equal a person's name (M17); every
     view shows the person beside an agent's name.
   - *Of an agent by a grant:* `X-Clax-Session` must name a live session
     of the signing grant; a grant cannot act for another grant's session
     (403 `foreign_session`).
   - *Of the owner by a person's machine:* a grant is never the owner, even
     an owner's own grant (§6.1); owner routes take owner credentials
     only, which never leave the owner's machine.
   - *Of a person in the browser:* the person's browser is a `clax_viewer`
     cookie over HTTP (§21.2).
2. **Token theft on the LAN.** Agent requests are signed (M4): a sniffed
   request replays only within 120 s and never twice (nonces), a sniffed
   token signs nothing. What is not protected: confidentiality (responses
   cross the LAN in clear), and a person's browser cookie, which a sniffer
   can reuse to act as that person in the browser: read their inbox,
   answer their agents' questions, approve content for their agents. The
   owner's browser credentials are loopback-only, so this never reaches the
   owner. People says so on the invite screen; Q3 asks about TLS.
3. **Invite and request races.** An invite code typed by Sam crosses the
   LAN in clear inside a signed request; a sniffer racing it redeems first
   with its own key. Sam then gets `invalid_invite`, and the owner sees a
   connection from an unexpected machine (key fingerprint, peer IP) in the
   inbox and People, and revokes it. An ad-hoc request is approved only
   with the code shown on the requesting machine (§6.3), so the owner
   cannot be tricked into approving a lookalike request.
4. **Confused deputy via page-written content.** A page's comments,
   documents and self-published versions carry its publisher as an author
   (M8), so a page published by Sam's agent cannot speak to the owner's
   agents through the owner's browser. A page cannot route (§10). A page
   cannot approve (approvals are person routes with the viewer `Origin`
   rules; the `comments` capability cannot reach them).
5. **Laundering.** An approved author who quotes an unapproved one carries
   the quote; content an agent writes after reading becomes its person's.
   The filter is about who wrote what reaches an agent, not about text
   flow inside agents. Known limitation.
6. **Replay.** Nonces per key for 300 s against a ±120 s window; grant
   requests and continuation calls are signed the same way; one-time
   codes (invite, browser link) are deleted on use.
7. **Local agents.** The owner's local agents run as the owner's OS user
   and can read `~/.clax` directly; the filter guards what Clax hands to a
   well-behaved agent (injection), not a local agent that goes around it.
   Another person's agents have no such access: for them the filter is a
   boundary.
8. **Denial of service.** Limits: pending requests (3 per IP, 20 in all),
   wrong codes (3), open approval requests (100 per approver), nonces
   (4,096 per key), invites (20 live), grants (20 live per person).
9. **Untrusted text.** Client names, agent names, person names, the
   content of approval cards: rendered as text; the CLI escapes control and
   bidirectional characters; payloads quote names.
10. **Logs** never hold tokens, codes, signatures, keys or content.
11. No `unsafe`; no hard links.

## 22. Failure modes

| Situation | Behaviour |
|---|---|
| The remote daemon is down | Tools return `daemon_unreachable` naming the remote; hooks exit 0 silently; nothing starts a local daemon while `remote.json` exists. |
| Clock skew beyond 120 s | `stale_request` with the daemon's time; the client retries once with its offset; persistent skew is reported by `clax connect --status` and `clax doctor`. |
| Token expired while the machine slept | The client rotates through the manage URI if the grant lives (a retired-but-unexpired token suffices to sign the rotation); otherwise `clax connect` again. |
| Grant revoked mid-session | The next request is 401 `grant_revoked`; the shim's tools say "this machine was disconnected from <url>"; sessions already ended (§14). |
| Approval decided while the agent waits in `wait_for_feedback` | The held rows become deliverable and wake the wait. |
| Approval revoked while rows are being taken | One transaction decides: rows taken before see the approval, after do not. |
| Two agents take an untargeted thread at once | One transaction wins (M18); the other's rows are gone and its next read shows `taken_by`. |
| Owner deletes an artifact with open requests | Requests withdrawn; their items read "gone". |
| The person row is folded (claim) | `person`, `author`, `approver` columns follow the fold. |
| `remote.json` exists but the key is unreadable | Every command fails with `remote_key_unreadable` and how to reconnect; nothing falls back to a local daemon silently. |

## 23. Time to usable and performance

- **Solo installs:** the fast path (§8.7) costs one atomic load per agent
  request; no new query runs. The `/a/…` bootstrap and gallery are
  unchanged; first paint and comment-ready gates unaffected.
- **Budgets** (added to `scripts/perf-daemon-budget.json`, seeded with two
  connected people, 50 approvals each, 6,000 comments across 200 threads
  with mixed authors):
  - `vis_threads_ms` 15: median of `GET …/threads` for a 200-comment
    artifact as a person's agent (today's owner value plus at most 2 ms).
  - `vis_feedback_poll_ms` 5: median of the feedback poll with 20 rows,
    half withheld.
  - `sig_verify_us` 150: median signature verification inside the gateway
    (measured by a criterion-free micro test in the perf script).
  - The probe loads under `cheap_p95_ms` and `cheap_max_ms` run with a
    second, signed client polling.
- **Hot paths:** `Visibility::for_reader` one indexed query; the
  session-person map cached; `take_feedback` adds one hash lookup per row;
  the first-take owner write is in the take's existing transaction.
- **Bundles:** the `people` module is lazy (16 KiB budget); `gallery` and
  `artifact` grow by at most 512 bytes (the People link, owner chips);
  `extPanel` stays within 65,536.
- **Connecting** (time to usable for the second person): `clax connect`
  with an invite to "Connected" in under 2 s on the LAN (one key
  generation, one request); the person's first agent is usable on its
  first tool call (registration as today plus one signature).

## 24. Testing

- **clax-core:** rights parse, presets, intersect, union, `own`, levels
  (table tests); signature canonical string, verify, skew, nonce cache
  with a fake clock; grant, invite and token state machines; visibility
  for every row of §8.2 and every approval scope, revocation, the
  `*unconnected` class, the fast path; take with withheld rows and
  placeholders once; first-take ownership races; names (derivation,
  sanitising, collisions, reserved, release on end); migrations 22–24 from
  21 with a populated fixture; the inbox rebuild keeps `seq` and FTS
  matches; the delivery guard.
- **clax-server:** grant flow end to end (invite, request with code, wrong
  codes, expiry with a fake clock, continuation, rotation, revocation); the
  agent gateway (allowlist, rights, foreign session, Origin refused); the
  route guard; the visibility matrix; approvals and routing; handoff and
  `thread_taken`; lifecycle cleanup; person-scoped inbox, questions,
  agents; the extension gateway's new admissions.
- **clax-mcp / clax-cli / Pi:** remote mode signing and rotation against a
  real daemon; `clax connect` (invite, request, `--off`, `--status`);
  clip fetch and rewrite; `status {name}`; `ask {approval}`;
  `comments_handoff`.
- **web:** People page, approval and connection cards, agent strip,
  owner chips, Send menu; hostile names and content as text; Playwright end
  to end with two people (the second person's agent driven by a test client
  holding a grant, on loopback, which the daemon treats like any other
  machine), screenshots at desktop and phone width.
- **Golden:** the single-owner fixture's agent-facing outputs before and
  after the migrations are identical (§19).
- Fake clocks and injected durations everywhere; no fixed sleeps; the
  whole gate stays within about 2 minutes.

## 25. Known limitations

- No TLS: responses and person browser cookies are readable on the LAN
  (§21.2).
- Revocation cannot unread; laundering through an approved author is not
  tracked (§21.5).
- Remote Codex sessions have no tier 5; remote agents have no live pages.
- The filter does not stop a local agent of the owner's from reading
  `~/.clax` directly (§21.7).
- An agent's name is unique only among live sessions: an ended agent's
  name may be reused, so history shows names with their time.
- Approvals are per person: Sam cannot approve Mia's content for one of his
  agents and not another.

## 26. Open questions

Each has a recommendation; none blocks Tasks 1–8.

- **Q1. `*unconnected` on new installs.** Migration 23 gives every install
  the approval, so new installs also let LAN viewers' comments reach the
  owner's agents. Recommendation: keep it on (one rule for old and new
  installs), and show it on People with a plain switch ("Comments from LAN
  viewers who are not connected reach your agents without asking"); when
  the owner makes the first invite, People shows the switch once more
  beside the invite.
- **Q2. Thread approval and future content.** M10 makes a thread approval
  cover the author's later comments in that thread. The stricter reading
  ("this batch only") would ask again on every reply. Recommendation:
  keep M10; the card says "this thread, including later replies".
- **Q3. TLS on the LAN.** Signatures protect agent requests, not
  confidentiality or person browser cookies. Recommendation: not in this
  round; document "use a trusted LAN"; a later design can add a
  self-signed certificate pinned by `clax connect` (the invite carries the
  fingerprint), which would also protect the browser.
- **Q4. Owner approving on a person's behalf.** O3 says the agent's person
  approves; the owner cannot. Recommendation: keep it so (M6); the owner
  can still give rights and see every request's state in People.
- **Q5. Untargeted fan-out among the owner's own agents (M18, §19).**
  Recommendation: accept first-take (O4's one owner); the Send menu already
  targets one agent whenever one is live, so the change only shows in the
  rare untargeted sends.
- **Q6. Agent names across machines.** Two people's agents in the same repo
  and branch both derive `clax-main`; the second becomes
  `clax-main-claude` or `clax-main-2`. Recommendation: accept; showing
  the person beside the name disambiguates for people, and `@name` stays
  unique. An alternative, `<person>/<repo>`, makes every name longer for
  the rare case.
- **Q7. The extension for other people.** The extension pairs with the
  local daemon by native messaging; another person's extension would pair
  with their own daemon, not the owner's. Recommendation: out of scope;
  other people use the shell over the LAN.
- **Q8. Approvals for titles in `list`.** Withholding titles makes a
  person's agent's `list` show "an artifact by Mia" for most pages until
  approved. Recommendation: keep (titles are author text and a short
  injection surface); `clax:collaborator`'s connect-time owner approval
  (M12) covers the common case.
- **Q9. UI design questions, to settle in one batch before Task 17 of the plan:**
  (a) People as its own page (recommended) or a tab of the inbox; (b) the
  approval card's buttons: four buttons (recommended) or "Allow ▾" with
  scopes in a menu; (c) the agent strip: chips above the inbox list
  (recommended) or a left rail on wide screens; (d) whether withheld
  content shows to the approver expanded (recommended: first 3 lines, then
  "Show all") or collapsed; (e) the owner chip on thread cards: name only
  (recommended) or name and person always.
