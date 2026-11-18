# Zero-capability sessions: one GNAP grant model for every permission

Date: 2026-11-17. Status: design, for the owner's review. Not built.
Every choice marked **(call)** was made without the owner and is theirs to
overturn; the questions left open are qualifier records on this file
tagged `status:needs-decision`.

## Intent

The owner's direction, in their words:

1. "Our whole permissions system, for everything, should use a consistent
   GNAP-based model where a session by default has zero capabilities and
   capabilities are proven to it in the appropriate state machine."
2. "If the proof of grant is revoked, then that capability is revisited."
3. "Effectively the capabilities of a session are the union of
   capabilities of all grants."
4. "Per our standard authorization, I should be able to grant or revoke
   those accesses at any time to any token of my choosing if I have those
   permissions myself."

Today four access types (`clax-artifacts`, `clax-comments`, `clax-db`,
`clax-ask`) are GNAP access objects, but a new session starts at its
person's bound, and every other permission (the daemon token, browser
views, the extension, links, queues, page capabilities, admin, ClaxBot)
is a separate check with its own credential, its own implicit allowance
and its own revocation story. This design puts all of them in one graph
of grants, evaluated one way.

## Goals

- Every authorization decision in Clax is one question: does the
  requesting holder's capability set contain this access? Nothing else
  answers it.
- A holder (a session, token, browser session, link, credential) starts
  with nothing. Everything it may do traces to a recorded grant.
- Revoking a grant, or a granter losing a capability, takes effect before
  the revoking request returns, for requests and for state held under it.
- One page shows, for any holder you control, every capability it has and
  the grant that gives it, and lets you grant or revoke within your own.
- The move from today's model locks no one out and changes no one's
  effective rights on the day each stage lands.

## Non-goals

- No new credential formats. Holders keep their tokens (`cxg_`, `cxa_`,
  `cxe_`, `cxv_`, cookies, signatures); what changes is what a token's
  access is computed from.
- No change to the visibility filter's rules (withheld authors) in the
  first stages; see the open question on folding it in.
- No external policy engine. The graph is small and lives in the store.
- No self-contained tokens. Every token is a reference the store resolves.

## Anti-goals

- A second place that decides. A route that checks a role, a flag, a
  credential prefix or a hard-coded denial list after this lands is a bug.
- Implicit defaults. A preset is only a name for an access array that an
  explicit grant carries; a template is itself a grant.
- Stale allowance. No cache, socket, lease or watch outlives the revoke
  of what it was granted under.

## What exists (verified in the code)

Request order: `crates/clax-server/src/router.rs` `wrap` runs the queue
reporter router first, then (local) `local_trust::sessions`,
`identity::local_peers`, `auth::require_api_host` or (hosted)
`hosted::gate`; then `local_trust::gate`, `auth::require_credential`,
`agent::authenticate`; per route `extension::gateway`,
`live_access::hide_live_pages`, `db_caller::refuse_reader_elsewhere`,
`agent::verify_layer`; last the handler extractors (`Identity`, `Caller`,
`Agent<N>`, `TokenOrAgent<N>`, `DeniedToGrants`, `RequireToken`,
`SameOrigin`, `Confirming`, `DaemonToken`). Only a machine grant's or an
agent token's caller is checked against `Rights`
(`clax_core::rights::effective`: `(person ∩ grant narrow ∩ session
narrow) ∪ capped raises`, computed once per request in
`clax_store::people::grant_caller_in` and re-read by long-polls when the
visibility generation moves). Every other path below decides by caller
kind, role or credential.

What the inventory turned up that this design must fix or carry:

| Finding | Where |
|---|---|
| The daemon token is never checked against rights; the owner's agents differ from the owner only by handlers that call `refuse_agents()` (`not_for_agents`). | `crates/clax-http/src/identity.rs` `caller()`, `crates/clax-http/src/agent.rs:1303` |
| A narrowing or rights change releases no queue lease or queue watch; queue access for agents is membership plus `clax-comments` somewhere. | `crates/clax-store/src/queues.rs` `queue_role`, `crates/clax-http/src/agent.rs` `reads_reports` |
| ClaxBot runs a revoked grant started keep running (each write is refused). | `crates/clax-http/src/bot.rs` `check_bot_write_in` |
| Held sockets re-verify their session only on reopen; room sockets fix their db level at open. | `crates/clax-http/src/identity.rs` `reopen` |
| `cxs_` names both an agent session secret and a hosted browser cookie; the Worker's stand-in git token is `cxm_…`, the grant manage prefix. | `crates/clax-core/src/secret.rs:22`, `crates/clax-store/src/hosted.rs:54`, `crates/clax-worker/src/repos/tokens.rs:165` |
| Owner browser logins, extension credential mint and revoke, reporter-token mint and device approvals write no audit record. | `crates/clax-core/src/audit/mod.rs` `kinds!` |
| `feat/token-queue-rights` points at main (`72bd4e8ca`) with no commits yet; no `clax-queues` type exists. This design fixes the type's shape so that branch lands as stage 2. | `git log feat/token-queue-rights` |

### Inventory of authorization paths

43 paths. "Proving grant" names the edge that proves the access under
this design; "L"/"H" mark local-only and hosted-only paths. Rows 3, 41
and 43 are a condition, holder-proof hygiene and diagnostics, listed for
completeness.

| # | Path | Credential | Allows today (implicit?) | Access type(s) | Proving grant | Issues / revokes |
|---|---|---|---|---|---|---|
| 1 | Daemon token, by hand (L) | `Bearer <64 hex>` in `daemon.json`, new each start | Everything; rights never consulted (implicit) | all but `clax-people`, `clax-grants`, `clax-views`, `clax-admin` | owner → local machine (`install`) | owner at `clax init`; `clax uninit`, owner in Access |
| 2 | Daemon token as the owner's agent (L) | token + `X-Clax-Session`/`X-Clax-Channel` | Everything but `not_for_agents` handlers (implicit) | per the local template | local template → session child | template's granter; session end |
| 3 | Confirmation gate (L) | token, no browser, an owner agent live | Six decisions wait for an owner browser click | a grant condition `confirm: browser` on `clax-people`, `clax-grants`, `clax-admin` actions | — | — |
| 4 | Login code (L) | `cxi_`, 120 s, one use | Opens an owner browser session | `clax-sessions` `register` (browser) | owner → local machine | token holder; spent on use |
| 5 | Owner browser session (L) | `cxb_` cookie + `cxp_` proof, 400 days | Owner on every route; `GET /api/token` returns the daemon token (implicit) | browser template | owner → browser (`signin`) | owner; sign-out, retire |
| 6 | In-process app frame (L) | in-process mark | Owner browser; answers confirmations (implicit) | browser template | owner → app frame (`install`) | owner; app removal |
| 7 | Extension credential (L) | `Clax-Extension cxe_`, 30 days idle | Owner identity on an allow-list of live-page routes | `clax-live` | owner → extension (`extension`) | owner; `DELETE /api/extension/credentials`, idle |
| 8 | LAN viewer cookie | `clax_viewer=cxv_`, self-minted | Read and comment where visible; db `view`/`interact` (implicit) | `clax-artifacts` `read`, `clax-comments` `write` `reply` `send`, `clax-db` | owner → `viewers_on:<bind>` (open question) | owner |
| 9 | Person's browser (L) | `cxv_` set by `/connect/b/<code>` | Inbox, approvals, own agents' narrow and raise, queues | browser template | person → browser (`signin`) | person; owner revokes person |
| 10 | Grant request and continuation | signed `POST /api/grants`; `GNAP cxc_` | Poll, collect tokens once, disconnect, owner-content answer | the GNAP flow itself | — | client; owner approves or denies |
| 11 | Grant manage token | `GNAP cxm_` | Revoke the whole grant | none: ending oneself is not a capability **(call)** | — | the person |
| 12 | Machine grant's agents | `Clax-Grant cxg_` + signature | ~100 routes in `agent::ROUTES`, each with its right | `clax-artifacts` `clax-comments` `clax-db` `clax-ask` `clax-sessions` | person → machine; template → session child | person, owner, 30-day idle |
| 13 | Agent session secret | `X-Clax-Session-Secret cxs_` | Act as one session; sessionless = ∩ of live narrowings | holder proof | template → session child | session end |
| 14 | Hosted agent token (H) | `Bearer cxa_` | Same routes as #12, person ∩ narrowing, hosted reach | as #12 | person → agent token | person, owner, self-revoke |
| 15 | Person's device (H) | `cxa_` from device sign-in | 13 `PERSON_DEVICE_ROUTES`, acting as the person's browser (implicit) | `clax-artifacts` `administer` (share, link), `clax-comments` | person → device (`signin`) | person, owner |
| 16 | Device code (H) | `cxd_` + user code, 10 min | Collect the `cxa_` once after approval | the GNAP flow | — | member approves; expiry |
| 17 | Hosted browser session (H) | `cxs_` cookie, 30 days | Owner within reach; person rights + shares; guest per link (implicit by role) | browser template | owner or person → browser; link → guest | sign-out, person revoke |
| 18 | Hosted owner shell token (H) | `cxo_` = HMAC(token, cookie) | Acts as the daemon token beside that cookie | browser template | browser → shell token (parent) | ends with the session |
| 19 | Hosted daemon token on a local Host (H) | `CLAX_TOKEN` | Owner within reach | local template | owner → deployment holder (`install`) | redeploy |
| 20 | Share link and guest (H) | artifact `LinkAccess`; guest viewer row | Read or comment to the link's level; git read token | `clax-artifacts` `read`, `clax-comments`, `clax-git` `fetch` | maker → `bearers_of:<link>` | maker |
| 21 | Per-person share (H) | share row | Rights at one artifact for a person and their agents | as shared | maker → person | maker |
| 22 | Site session | site cookie via `/site-signin` code, 1 h idle | Page runtime `/_clax/api/*` as the holder | the holder's, at that artifact | holder → site session (parent) | ends with parent; expiry |
| 23 | Site visitor (world-open site) | visitor cookie | Read the artifact (implicit when open) | `clax-artifacts` `read` | maker → `visitors_of:<site>` | maker closes the site |
| 24 | db caller level | derived from #1, #5, #8 | token `owner`, browser `admin`, named viewer `interact`, else `view` (implicit) | `clax-db` `level` | the holder's grants | — |
| 25 | App-to-app db read | `X-Clax-Reader: <A>` | Read source docs at most at `view` | `clax-page` `read-docs` at the source | owner or maker → page A | owner or maker |
| 26 | Page capability approval | declared `capabilities` | Gated capability once approved; `net` also needs `net-grants` | `clax-page` | answerer → page runtime | owner (L), maker (H) |
| 27 | Agent read consents | consent row | Remembered consent for agents' cross-app reads | `clax-page` `read-docs` on agent holders | person → `agents_of:<person>` | person, owner |
| 28 | Live pages and mirrors | caller kind | Live pages to the owner and the extension; mirrors to the owner and their agents (implicit) | `clax-live` `read` | browser template; extension grant | owner |
| 29 | Queue service grant | `Clax-Queue cxq_` + signature | Mint reporter tokens on its queue | `clax-queues` `mint-reporter` | queue manager → service | queue manager |
| 30 | Reporter token | `Clax-Reporter cxr_` + signature + Origin, 900 s | File, list, comment, verify, reopen own reports | `clax-queues` `report` | service → reporter (parent) | expiry, service revoke |
| 31 | Queue membership | `queue_members` role | Triage members and all their agents list, claim, reply, fix, resolve (implicit for agents) | `clax-queues` | queue manager → person; template child carries it | queue manager |
| 32 | Queue lease | `lease_id` + fence | Write to a report only under a live lease | state under `clax-queues` `claim` | — | ends on loss of `claim` |
| 33 | People and grant administration | owner browser + SameOrigin + confirmation | Invites, approve, revoke, rights; persons narrow and raise their agents | `clax-people`, `clax-grants` | root; browser template | owner |
| 34 | Visibility approvals | person's browser | Which authors reach the person's agents | `clax-authors` (open question) | person → `agents_of:<person>` | person |
| 35 | Admin role and ClaxBot | owner or `admin` role; flags; credits | Admin pages, flags, credits, provider, settings; bot runs | `clax-admin`, `clax-bot` | owner → person | owner |
| 36 | ClaxBot surfaces host (L) | daemon token | Register surfaces, start runs, serve | `clax-bot` `register` `serve` | owner → local machine | owner |
| 37 | MCP | token (`/mcp`); token or `cxa_` (`/mcp/json`) | Tools call REST as the caller; no escalation | the caller's | the caller's | — |
| 38 | Git repo tokens | Worker `cxm_<scope>_…` or Cloudflare tokens, 900–3600 s | Clone, fetch; push = publish | `clax-git` | holder → git token (parent) | ends with parent; host revoke list |
| 39 | Jobs owner claim | in-memory token tied to a stream | Single writer of a client's jobs | `clax-views` `jobs` | browser template | stream end |
| 40 | Draft token | 22-character capability URL | Serve an agent's draft files | `clax-artifacts` `read` at the draft | session → `bearers_of:<draft>` | draft replaced |
| 41 | Host, DNS-rebind, CSRF, origin checks | — | Refuse wrong `Host`, cross-origin writes, signed requests with cookies | stays as holder-proof hygiene | — | — |
| 42 | No credential | — | `healthz`, `/.well-known/clax`, `/_clax/*`, content by ID, site hosts | `clax-artifacts` `read` for content by ID (open question) | maker → `bearers_of:<artifact ID>`, or none | — |
| 43 | Worker diagnostics | `CLAX_TOKEN` while `CLAX_DIAG_ROUTES=1` | `/_diag/*` | `clax-admin` `diag` | root → deployment holder | redeploy |

## The model

### Terms

| Term | Meaning |
|---|---|
| **Principal** | Who answers for authority: the **instance** (the install itself), the **owner**, a **person**, a **service** (a queue, a ClaxBot surface registration, a remote). |
| **Holder** | What presents a proof on a request: a browser session, the daemon token, a machine grant, an agent session, an agent token, an extension credential, a viewer link, a queue or reporter credential, a git token, a site session, an artifact's page runtime. Every holder has one **controller**, the principal that answers for it (a session's person, a link's maker, a page's maker). |
| **Access object** | A GNAP (RFC 9635 §8) object `{type, actions, locations?, ...}`, as in `crates/clax-core/src/rights.rs` today, with the types listed below. |
| **Grant** | An edge `granter → grantee` carrying an access array: `{id, granter, grantee, access, parent?, conditions, status, version, issued_at, event}`. The granter is a principal or a holder; the grantee is a principal, a holder, or a holder **selector** (below). |
| **Selector grant** | A grant whose grantee is a set named by a rule fixed at issue: `sessions_of:<machine>`, `members_of:<queue>`, `bearers_of:<link>`, `agents_of:<person>`. It conveys nothing itself. When a matching holder appears, it issues that holder a **child** grant (`parent` = the selector grant, same access, journaled). This is how a default is made explicit: issued once at a recorded event, visible, revocable. |
| **Template** | A selector grant from a person to `sessions_of:<their machine>` (or the owner's local machine). It replaces "a new session starts at the bound": narrowing the machine is editing the template, narrowing one agent is editing its child. |
| **Proof** | Two things per request: the **holder proof** (the credential, checked as today) and the **capability proof** (a live chain of grants from the instance to the holder containing the access, found by the evaluator). |

### Access types

| Type | Actions | Locations | Replaces |
|---|---|---|---|
| `clax-artifacts` | `read` `create` `update` `delete` `administer` (pin, tags, capabilities, share, link) | `*`, `own`, artifact IDs | today's type, plus `admin_only` checks |
| `clax-comments` | `read` `write` `reply` `resolve` `send` `watch` | as above | today's type |
| `clax-db` | `read` `write`, `level` | as above | today's type |
| `clax-ask` | `ask` | none | today's type |
| `clax-queues` | `report` `read` `triage` `claim` `fix` `verify` `manage` `mint-reporter` **(call: to be aligned with `feat/token-queue-rights`)** | queue IDs, `*` | queue membership roles, service and reporter credentials |
| `clax-sessions` | `register` `read` `end` | `own` | the daemon token's and a grant's right to register sessions |
| `clax-views` | `inbox` `threads` `stream` `people` `audit` `events` `drafts` `jobs` | none | the `not_for_agents` denials (browser views) |
| `clax-live` | `read` `comment` `rules` `join` | origins, `*` | the extension gateway's route list, live-page visibility |
| `clax-page` | `net` (hosts) `sample` `shell` `notify` `share` `file` `camera` `location` `account` `read-docs` (source artifacts) | the page's artifact ID | capability approvals, `net-grants`, `sample-grant`, `db-grants` |
| `clax-people` | `read` `invite` `approve` `rights` `revoke` `reinstate` | person IDs, `*` | `browser_required` owner routes |
| `clax-grants` | `read` `issue` `revoke` | holder IDs, `own` (holders the caller controls), `*` | who may change rights, raises and narrowings |
| `clax-admin` | `roles` `flags` `credits` `pricing` `provider` `usage` `audit` `web` `shutdown` `perf` `settings` `diag` | none | admin role, owner-only settings, Worker diagnostics |
| `clax-bot` | `run` `register` `serve` `cancel`, `credits` (a weekly limit) | surface names, `*` | the ClaxBot flag and credits checks |
| `clax-remotes` | `connect` `read` `push` `manage` | remote IDs, origins, `*` | remote connection requests, push |
| `clax-git` | `fetch` `push` | artifact IDs | git tokens (derived) |
| `clax-authors` | `read` | author IDs × artifact locations | visibility approvals (only if the open question folds them in) |

`clax-grants` is what makes the owner's fourth point safe: holding an
access lets you pass it on only if you also hold `clax-grants` `issue` on
the target holder. The browser-session template carries it; the agent
session template does not, so "no agent changes rights" survives as an
absent grant instead of a hard-coded refusal **(call)**.

### The evaluation rule

```
caps(instance) = ⊤                                    (every type, every action, *)
caps(x)        = ∪ { conv(g) ∩ caps(g.granter)  :  g live, g.grantee = x }
conv(g)        = g.access ∩ conv(g.parent)            (g.access when g has no parent)
```

taken as the **least** fixed point. Equivalently: `x` holds access `a`
exactly when there is a path `instance → … → x` of live grants each of
whose conveyed access (`conv`) contains `a`. Consequences:

| Property | Why it holds |
|---|---|
| A holder with no grant reaching it holds nothing. | Least fixed point; no base case but the instance. |
| A cycle of grants with no path from the instance conveys nothing. | Same. |
| Granting never widens past the granter. | Intersection with `caps(granter)` at every edge. |
| A granter's loss shrinks every grant they issued, at once, without rewriting them. | `caps(g.granter)` is read at evaluation, not copied at issue. |
| Union across grants: two partial grants combine. | `∪` over grants. |
| `own` means the grantee's person. | Stored as `own:<person>` at issue (as `rights.rs` does today), then intersected like any location. |
| Narrowing is editing a grant you issued; there are no negative grants. | The granter `PATCH`es its own grant's access (with `if_version`). |

A grant is **live** while: its status is `issued`; its `parent` (if any) is
live; its conditions hold (`expires_at`, idle limit, the grantee holder
still exists); its granter, if a holder, still exists.

### Revocation and re-evaluation

| Trigger | Effect |
|---|---|
| A grant is revoked (`DELETE`), expires, idles out, or its parent ends | The grant is no longer live; every holder downstream is re-evaluated. |
| A granter loses an access (any of the above upstream of it) | Each grant it issued conveys less; no record changes. If it regains the access, the grant conveys it again **(call; open question)**. |
| A person is revoked | Every grant whose granter or grantee is that person or one of their holders ends (status `revoked`, reason `person_revoked`). |
| A holder ends (session end, `clax connect --off`, link deleted) | Grants it holds or issued end with it. |

Re-evaluation runs in the transaction's after-commit step and, before the
revoking request answers, ends state that the holder may no longer keep:

| State held under a capability | On loss |
|---|---|
| Watches (`clax-comments` `watch`) | Stopped; `watch.stop` with `cause: grant`. |
| Queue leases (`clax-queues` `claim`) | Released; `queue.release` `reason: grant_revoked`. |
| Working records | Ended; `working.stop` `reason: grant_revoked`. |
| Stream subscriptions (`/api/stream`, SSE, websockets, rooms) | Each topic the holder no longer reads is unsubscribed with an `access_ended` event; a stream whose holder holds nothing is closed. |
| Pending sends and undelivered feedback | Held, as `no_access` threads are today. |
| Open questions and approval requests (`clax-ask`) | Withdrawn, `reason: grant_revoked`. |
| Drafts, git tokens, site sessions, page runtime grants | Invalidated; the next use is 401 or 403. |
| ClaxBot runs acting for the holder | Stopped, `bot.run_end` `reason: grant_revoked`. |
| Live-page streams opened by the extension | Closed (as today on credential revoke). |

### Checking a request cheaply, never stale

| Step | Cost |
|---|---|
| 1. Holder proof (token hash lookup, signature, cookie and proof), as today. | One indexed read. |
| 2. Read the store's `grants_epoch` (one integer row, bumped in the same transaction as every grant change, person revoke, artifact create or delete that moves an `own`, and holder end). | In memory on a single daemon; one read on a deployment with several processes. |
| 3. If the holder's cached capability set carries that epoch, use it; else recompute it by walking grants backwards from the holder (indexed by grantee and selector), memoising each granter's set, and cache it with the epoch. | Graph depth is at most about 5 (instance → owner → person → machine → session → git token); grant counts are hundreds to low thousands. |
| 4. `allows(type, action, location)` on the set, as `rights.rs` does. | Pure. |

The epoch is bumped before the writer's transaction commits and the cache
is checked against it on every request, so a request that starts after a
revoke's commit sees the revoke. Long-lived state is swept by the
re-evaluation step above, which runs before the revoke answers. A refusal
names what was missing, as today (`403 not_permitted` with `right: {type,
action}`), and adds `request: "/api/access/requests"` so the client can
ask for it.

### One token per holder; grants are edges

A holder's token is its proof of being that holder; its access is not
fixed at issue. RFC 9635 lets the authorization server change what a
reference token conveys; Clax is its own authorization server and
resource server, so every token is a reference and its access is the
union of live grants reaching its holder. Adding a grant to a holder
issues no new token; revoking one revokes no token. Revoking a token
(`DELETE /api/grants/<gid>/token`) ends the holder, and with it every
grant it holds or issued.

### The state machines

Every grant passes through RFC 9635's states. Clax names them:

```
requested ──(needs a person)──> pending ──approve──> approved ──issue──> issued ──> revoked | expired
     │                              └──deny / lapse──> denied              │
     └──(granter-initiated, pre-approved)──────────────────────> issued    └─rotate─> issued (new token, same grant)
```

A granter-initiated grant (a raise, a share, a preset at setup) is a
request the granter makes for the grantee, approved by being the
granter's own act. A holder-initiated grant is a GNAP request the holder
signs with its existing proof (`POST /api/access/requests {access,
interact: {start: ["clax"]}}`), shown to the controller or a granter who
could issue it, approved or denied there.

| Grant kind | Request | Interaction / approval | Issue | Continuation | Rotation | Revocation |
|---|---|---|---|---|---|---|
| Instance → owner (root) | None: issued at install (`clax init`, or a hosted deploy's first owner sign-in) | The install is the act | Root grant, access ⊤ (hosted: locations `own`) | None | Owner transfer only (`clax access owner`, OS user) | Never by request; see risks |
| Owner → person | Invite (`POST /api/people/invites`) or connect request | Invite: pre-approved; request: owner enters `user_code` | Person grant with the chosen preset | `cxc_` on `/api/grants/<gid>/continue` | n/a | Owner (`DELETE /api/people/<pid>`) |
| Person → machine | `POST /api/grants` signed with the machine key | Invite code, or owner approval | Machine grant + `cxg_`/`cxm_`; plus the machine's template | `cxc_` | New: `POST /api/grants/<gid>/token` with the manage token (RFC 9635 §6.1) **(call)** | Person (`--off`), owner, manage token |
| Template → agent session | Session registration under a holder with `clax-sessions` `register` | None (the template is the standing approval) | Session reached by the template's selector; `cxs_` proves it | Session secret | Re-registration | Session end, person, owner |
| Granter → session (raise) | Granter's `PUT`, or the session's `POST /api/access/requests` | Granter's act, or the granter approving the request | Grant on the session | Request's continuation | n/a | Granter, session's person, owner |
| Person → browser session | Sign-in (login link, passkey, GitHub, `/connect/b/<code>`) | The sign-in proof | Browser template grant (all the person holds, with `clax-grants`) | Cookie + proof | Cookie rotation as today | Sign-out, `browser-login/retire`, person revoke |
| Browser → shell token | `GET /api/token` / hosted shell token | Owner browser session | Derived grant, same access | None | Per page load | Ends with the browser session |
| Owner → daemon token (local machine) | `clax init` | The OS user | Local machine grant + its session template | None | Token is new at every daemon start (as today); the grant persists | `clax uninit`; see open question |
| Person → agent token (`cxa_`) | Browser `POST` | The person's browser | Token grant | None | Revoke and mint | Person, owner |
| Owner → extension (`cxe_`) | `clax extension` / `POST /api/extension/credentials` | Owner browser or native host | `clax-live` grant | None | Re-pair | Owner (`DELETE /api/extension/credentials`), idle |
| Maker → link (`cxv_`) | Maker sets a link | Maker's act | Selector grant `bearers_of:<link>` | None | New link | Maker |
| Link → guest browser | Guest opens the link | Link secret | Guest browser session grant | Cookie | — | Ends with the link |
| Maker → person (share, hosted) | Maker's share | Maker's act | Grant at the artifact | None | — | Maker, person revoke |
| Queue → member, service, reporter | Queue manager's act (members, service); service's mint (reporter) | Queue manager | `clax-queues` grant; `cxq_`/`cxr_` | None | Reissue | Queue manager, owner |
| Answerer → page (`clax-page`) | Page declares a capability | Owner (local) or maker (hosted) approves in Clax | Grant on the page runtime holder | None | Version bump re-asks when the declaration widens | Answerer, owner |
| Owner → person (admin, ClaxBot) | Owner in Admin | Owner's act | `clax-admin`, `clax-bot` grants | None | — | Owner |
| Remote connection | Agent's `remote_request.ask` | Owner answers | `clax-remotes` grant + the remote's own GNAP grant to this instance | Device code | Per remote | Owner |
| Holder → git token | `POST /api/sessions/<id>/git-token/<aid>` | None (sub-delegation) | `clax-git` grant, parent = the session's | None | Per use | Ends with the parent |

## Bootstrap: every first capability is a recorded grant

No holder gets anything from being what it is. Each row is the recorded
event that issues the first grant, with what it carries by default; every
default is a preset the issuer can change at that moment or later.

| Who | Event (journal) | Grant issued | Default access |
|---|---|---|---|
| The owner | `clax init` on a new home, or a hosted deploy's first owner sign-in (`grant.issue` `event: install`) | instance → owner (root) | ⊤ (hosted: every type at `own`, as the contract's reach rule says today) |
| The owner's browser | Owner sign-in (`signin`) | owner → browser session | Browser template: all the owner holds |
| The owner's local machine | `clax init` (`install`) | owner → daemon token holder; owner → `sessions_of:<local machine>` (template) | Local template: all but `clax-people`, `clax-grants`, `clax-admin`, `clax-views` (today's `browser_required` and `not_for_agents`) |
| A local agent session | Session registration (`session`) | Reached by the local template | The template's access |
| A new person | Invite accepted or connect request approved (`invite`/`connect`) | owner → person | `clax:collaborator` (today's default) |
| A person's browser | `/connect/b/<code>`, login link, GitHub (`signin`) | person → browser session | Browser template: all the person holds, with `clax-grants` on `own` holders |
| A person's machine | Grant approved (`connect`) | person → machine; person → `sessions_of:<machine>` | Machine: all the person holds but `clax-grants`, `clax-people`, `clax-views`; template: the same |
| A person's agent session | Registration under the machine (`session`) | Reached by the machine's template | The template's access |
| The browser shell (hosted shell token, `/api/token`) | Page load from a signed-in browser (`signin`) | browser → shell token (parent = browser grant) | Same as the browser |
| A guest (viewer link) | Maker creates the link (`link`); guest opens it (`signin`) | maker → `bearers_of:<link>`; link → guest browser | `clax-artifacts` `read`, `clax-comments` `read` `write` `reply` at that artifact |
| A LAN viewer (not connected) | Owner turns on LAN serving (`clax serve --bind`, recorded) **(open question)** | owner → `viewers_on:<bind>` | Today's viewer routes: read, comment, send |
| The extension | Pairing (`POST /api/extension/credentials`, owner browser or native host) | owner → extension credential | `clax-live` all actions, `*` origins |
| A queue's service backend | Queue manager issues a service credential (`queue`) | queue manager → `cxq_` holder | `clax-queues` `mint-reporter` at that queue |
| A reporter | Queue manager or service mints a reporter token (`queue`) | → `cxr_` holder | `clax-queues` `report` at that queue |
| A ClaxBot surface | Registration (`bot`) | registrant → surface; person → run (per run) | The surface's declared access ∩ the person it acts for |
| A page's gated capabilities | Answerer approves in Clax (`page`) | answerer → page runtime | The approved capabilities only |
| A remote instance | Owner answers a connection request (`remote`) | owner → remote holder | `clax-remotes` `read` (and `push` when asked) |
| An admin | Owner grants the role (`admin`) | owner → person | `clax-admin` chosen actions |

## The access page

One place: **Settings, Access** (`/settings/access`), and the same panel
opened from wherever a holder appears (an agent chip, a People row, the
extension's entry, a link's menu, a queue's members, a page's capability
prompt). It is a browser view (`clax-views`); agents get the same data by
API only when they hold `clax-grants` `read`.

| Part | Shows | Acts |
|---|---|---|
| Holder list | Every holder you control or hold `clax-grants` `read` on, grouped by principal (you, each person, services), with live/ended status | Filter by type, by "granted by me" |
| Holder panel | Effective capabilities, grouped by access type; each line lists the grants that give it (granter, event, when) and the chain to the instance on hover | Revoke a grant; edit (narrow) one you issued; end the holder |
| Grant form | Holder, access type, actions, locations, from a preset or by hand; expiry | Every option you do not hold is shown disabled with "you don't hold this"; submit issues `POST /api/access/grants` |
| Requests | Pending holder-initiated requests you can approve | Approve (optionally narrower), deny |
| Granted by me | Every live grant you issued, with what it conveys now (less than issued when you lost something) | Revoke, edit |

Routes (browser, same-origin, need `clax-grants`):

| Route | Does |
|---|---|
| `GET /api/access/holders` | Holders in reach of the caller. |
| `GET /api/access/holders/<hid>` | Effective set, grants, chains, live state held. |
| `POST /api/access/grants` | Issue: 403 `beyond_your_rights` naming the missing access. |
| `PATCH /api/access/grants/<id>` | Edit own grant; `if_version`; 400 `not_narrower` only for a template whose sessions are live **(call)**. |
| `DELETE /api/access/grants/<id>` | Revoke: granter, holder's controller, or owner. |
| `POST /api/access/requests` | A holder asks for access (GNAP request, signed with its proof). |
| `POST /api/access/requests/<rid>/approve`, `/deny` | Decide. |

`PUT /api/agents/<handle>/narrow`, `/elevate`, `PUT /api/people/<pid>/rights`
and `PUT /api/people/me/grants/<gid>/narrow` stay as thin aliases over these
routes until the contract drops them.

## Migration

Each stage is one access type (or group) moving from its old check to the
evaluator, behind a per-type switch in the store (`access_zero` set),
after a shadow run. Rolling a stage back flips its switch; grants stay.

| Stage | Steps |
|---|---|
| 0. Grant store and evaluator, shadow | Add the store, the evaluator and the access routes. Every request computes both decisions and counts differences per route and type (`/api/access/shadow`, owner only); the old decision answers. No behaviour changes. |
| 1. Backfill | One migration issues, as `grant.issue` with `event: migration` and actor `system:migration`, exactly what today's code implies (table below). Shadow differences must reach zero before stage 2. |
| 2. `clax-queues` | The first type built zero-by-default with union evaluation, on `feat/token-queue-rights`. That branch adopts this design's grant store, the type's shape above, and member, service and reporter grants as graph grants; leases and queue watches are swept on loss of `claim`, closing today's gap where a narrowing leaves them. |
| 3. `clax-artifacts`, `clax-comments`, `clax-db`, `clax-ask` for connected persons' agents | Sessions stop starting at the bound; each is reached by its machine's template (backfilled as the person's rights within the machine's narrowing). Narrowing becomes editing the template or a session's grant; raises become grants. |
| 4. `clax-views` | The `not_for_agents` and `forbidden` classes of the route table become `clax-views` and `clax-people` checks; the generated route table gains a `Needs` column naming the access instead of the class. |
| 5. The owner's local machine | The daemon token and local sessions move to the local template. Highest lockout risk: shipped last of the request paths, after a release of shadow running on the owner's own home. |
| 6. `clax-live`, `clax-page`, `clax-git`, `clax-remotes` | Extension, page capabilities, git tokens, remotes. |
| 7. `clax-people`, `clax-grants`, `clax-admin`, `clax-bot` | Owner and admin powers become grants from the root. |
| 8. Links, guests, LAN viewers, content addresses | Per the open question's answer. |
| 9. Remove the old checks | Delete each stage's fallback; a check outside the evaluator fails a lint (`//scripts:` gate). |

What stage 1 issues:

| From today | Grant issued |
|---|---|
| The install | instance → owner, ⊤ (hosted: `own`) |
| `daemon.json`'s token | owner → local machine, local template |
| Each live owner browser session | owner → browser, browser template |
| `PUT /api/people/<pid>/rights` value (or the default preset) | owner → person, that value |
| Each live machine grant and its narrowing | person → machine, the person's rights ∩ the narrowing; template the same |
| Each live session | the machine template's child for it, its access the session's narrowing (or the template's when none) |
| Each raise | granter → session, the raise as stored |
| Each live `cxa_`, `cxe_`, `cxv_`, `cxq_`, `cxr_` | as the bootstrap table, with today's access |
| Each share, `db-grants`, `net-grants`, `sample-grant`, capability approval | maker or owner → person or page, as stored |
| Each admin role, ClaxBot flag, credit lift | owner → person, `clax-admin` / `clax-bot` |
| Each approval of an author | unchanged (visibility filter), pending the open question |

## Journal records

The audit kinds (`crates/clax-core/src/audit/mod.rs`, `kinds!`) already
hold `grant.request`, `grant.approve`, `grant.deny`, `grant.revoke`,
`grant.token` and `grant.narrow` for machine grants. They widen to every
grant, and three are added:

| Kind | Body | Recorded at |
|---|---|---|
| `grant.request` (widened) | `request_id, holder, access, interact` | a holder asks for access (a machine connecting is one case) |
| `grant.approve`, `grant.deny` (widened) | `request_id, grant_id?, reason?` (`lapse` for a timeout) | a request decided or lapsed |
| `grant.issue` (new) | `grant_id, granter, grantee, access, parent?, event (install\|invite\|connect\|signin\|session\|raise\|share\|link\|extension\|queue\|page\|admin\|bot\|remote\|git\|migration), expires_at?` | every grant issued, templates and backfills included |
| `grant.update` (new; absorbs `grant.narrow`, `agent.narrow`, `agent.elevate`, `person.rights`) | `grant_id, from_access, to_access, version` | a granter edits a grant |
| `grant.token` (widened) | `grant_id, op (issue\|rotate), token_kind` | a holder's token issued or rotated |
| `grant.revoke` (widened) | `grant_id, by, reason (granter\|controller\|owner\|person_revoked\|holder_ended\|parent_ended\|expired\|idle)` | a grant ends |
| `grant.reevaluate` (new) | `holder, cause (grant_id), lost[access], regained[access], ended[{kind, id}]` (`system:grants`, `for_actor` the holder's controller) | a holder's effective set changed because of something upstream; one per holder per cause |

`admin.role`, `admin.flag`, `admin.credits`, `artifact.update`'s
`share` and `db_read_grants`, `queue.update`'s member and grant ops and
`remote_request.answer` also write the `grant.*` record for the grant
they change; old records stay readable. Owner browser logins, extension
credentials, reporter tokens and device approvals, recorded nowhere
today, record `grant.issue` and `grant.revoke`. Who reads them follows the audit view's
existing rule: an admin reads all; anyone else the records they acted in
or that name a holder they control.

## Risks

| Risk | Mitigation |
|---|---|
| Lockout: a bad stage leaves the owner without `clax-grants`. | The root grant cannot be revoked by request; `clax access repair` (OS user, daemon stopped) reissues the owner's browser template. Each stage is a per-type switch that falls back to the old check. |
| The daemon token is readable by any process of the owner's OS user, so an agent can drop its session header and act as the local machine holder. | Same as today (contract "Security model"); the local machine grant becomes narrowable, which is more than today. See the open question. |
| Re-evaluation fan-out after a high grant changes (the owner's root, a person) touches every holder. | Recompute is lazy per holder; only holders with live state are swept eagerly. Measured on demand (`just perf`). |
| Hosted deployments with more than one process see a stale in-memory epoch. | The epoch is read from the store on each request there (one indexed row). |
| Dynamic intersection surprises: access comes back when a granter regains it. | Shown in the holder panel ("conveys again since …"); open question. |
| Selector grants reach holders made later, which can feel implicit. | They are listed on the holder ("reached by template … issued at …") and in `grant.issue`. |
| The migration's backfill must equal today's effective rights exactly. | Shadow stage compares old and new decisions on every request and blocks the stage's switch until a run has no difference. |

## Alternatives considered

| Alternative | Why not now | Revisit |
|---|---|---|
| Keep the bound model and add types to it | Leaves implicit defaults and per-surface checks; fails the owner's first point | never, unless the owner withdraws the direction |
| Self-contained tokens (macaroons, Biscuit) with attenuation | Revocation needs a store lookup anyway; Clax is its own resource server | when a resource server runs apart from the store (Worker edge serving content without it) |
| Permanent cascade revoke instead of dynamic intersection | Loses the owner's "union of grants, revisited" semantics; restores are manual | if owners report surprise at access coming back |
| Negative grants or ceilings on a holder | Breaks pure union; today's "narrowing caps raises" becomes revoking the grant | if a person needs to refuse future raises on an agent |
| An external relationship engine (Zanzibar-style, OpenFGA, SpiceDB) | A service to run for a graph of thousands of edges | when grants exceed about 100k or authorization spans instances |
| Full graph walk per request, no cache | Simpler, but every request pays depth × fan-in | if profiling shows the cache costs more than it saves |
| Capability URLs (unguessable IDs) kept outside the graph | Keeps content addresses fast and shareable | the open question decides |

## Open questions

Recorded on this file as qualifier records tagged
`status:needs-decision`; `qualifier threads
docs/superpowers/specs/2026-11-17-zero-capability-gnap-design.md
--status needs-decision` lists them.

| # | Question | This design assumes |
|---|---|---|
| Q1 | When a granter loses an access and later regains it, do the grants they issued convey it again (dynamic), or are they cut for good (cascade revoke)? | Dynamic, per the evaluation rule. |
| Q2 | May a holder's controller set a ceiling that refuses future grants of some access to that holder (today a narrowing caps every raise)? | No ceilings: the controller revokes the grant, and the granter may grant again. |
| Q3 | Does the owner's daemon token become a grant like any other (narrowable, zero by default), or stay unlimited for the owner's OS user with only marked agent sessions under a template? | A grant, migrated last (stage 5). |
| Q4 | Do LAN viewers, world-open site visitors and capability URLs (content by artifact ID, `/_blob/`, drafts) enter the graph as selector grants, or stay bearer capabilities outside it? | Selector grants for LAN viewers and visitors; content by ID undecided (stage 8). |
| Q5 | May an agent session ever hold `clax-grants`, for example to pass a narrower set to a sub-agent? | No: only browser templates carry it. |
| Q6 | Who may revoke a grant they did not issue? The owner's fourth point reads as anyone who holds the access and `clax-grants` on the holder. | The granter, the holder's controller, and the owner only. |
| Q7 | Do visibility approvals (withheld authors) become a `clax-authors` access type in the graph? | Not in the first stages; the filter stays as it is. |
| Q8 | Are ClaxBot credits a limit carried on a `clax-bot` grant, or a ledger outside the graph that a grant only points to? | A ledger outside; the grant carries `credits` as a cap. |
| Q9 | Should grants and tokens expire or rotate by default (today access tokens neither expire nor rotate; grants end after 30 days unused)? | Keep today's rules; add rotation as an option. |
| Q10 | What evidence advances a stage: zero shadow differences over how long, on whose homes, approved by whom? | Zero differences over one release on the owner's home and clax.dev, owner approves each stage. |

## Left to the builder

- The store schema (`grants`, `grant_selectors`, `holders`,
  `grants_epoch`), the migration number, and the evaluator's crate
  placement (`clax-core` for the pure rule, `clax-store` for loading,
  `clax-http` for the extractor), within the layering in
  `scripts/check-crate-layers.py`.
- Tests: a property test that the evaluator equals the path definition on
  random graphs; per stage, a golden of old-versus-new decisions over the
  route table; revocation tests with an injected clock for expiry and
  idle, and exit conditions (no sleeps) for swept state.
