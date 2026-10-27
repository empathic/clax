# The Clax SDK, external artifacts and report queues

Status: draft for the owner's review (2026-10-27).

## 1. Intent

One SDK, in each language Clax ships, through which every client talks to
a Clax instance: a local daemon (`localhost:7480`), clax.dev (the Worker,
`crates/clax-worker`), or a Clax a deployer runs on their own
infrastructure (a daemon in hosted mode). Clax's own clients use it (the
shell, the Chrome extension, the Pi plugin, the CLI, hooks and MCP shim,
the Clax app), and so can a deployer's app.

The first use it is built for: a deployer integrates the SDK into their
app; each of their employees, signed in to that app and nothing else,
files bugs and other reports with the Clax selector (comment mode, picks,
clips) into a queue on the deployer's Clax; the deployer's agents read the
queue, claim reports, act on them and answer the reporters.

## 2. Owner decisions

- D1. The extension reaches clax.dev and a deployer's Clax, not only the
  local daemon.
- D2. The API contract is generated from the Rust routes, not written
  first.
- D3. Licensing is out of scope for this design.
- D4. A deployer runs our server on their own infrastructure (Kubernetes).
  Packaging and running it there is not part of this work.
- D5. Version 1 keeps the current identity systems: the owner on a local
  daemon, GitHub sign-in on a hosted Clax, people and agents through
  grants. No OIDC, no token exchange, no identities defined by a deployer.
- D6. The SDK takes an opaque identifier string that consumers of Clax can
  read, and an opaque metadata value beside it that consuming apps read,
  so integrations can extend what Clax records without Clax interpreting
  it (§6).
- D7. Clax keeps metadata for artifacts it does not store, extending what
  live pages do today for the extension on sites that are not Clax's (§6).
- D8. Who sees which reports in a queue is the deployer's choice per
  queue; the default is each reporter sees their own reports only (§7.2).
- D9. Reports always carry clips (§8.2).
- D10. A claim on a report is a lease that expires and is renewed; the
  default lease is 10 minutes, and `working` on a claimed report renews it
  (§7.4).
- D11. Employees sign in to the deployer's app and nothing else: filing a
  report takes no second sign-in or approval (§5.3).
- D12. Reporter tokens last 15 minutes and are renewed silently through
  the SDK's `auth` callback (§5.3).
- D13. A person with the triage right may also file into a queue directly
  from the Clax UI, as a person, with the existing thread model.
- D14. Credentials are of two kinds, browser session and signed grant;
  consent is given only in a browser session; the daemon token becomes a
  bootstrap; the extension's `cxe_` credential is replaced by a grant
  (§5.1, §5.2).
- D15. Anyone who may edit an artifact may write its metadata, and every
  change to it is recorded in the Toolpath journal (§6.6).
- D16. The opaque pair of D6 attaches to artifacts (`external_id`,
  `metadata`, §6.1) and to threads (`reporter_id`, `metadata`, §6.4).
- D17. An `external_id` names one artifact on an instance: two apps that
  send the same ID mean the same artifact. Integrators choose IDs unique
  for the objects they name; Clax does not namespace them (§6.1).

## 3. What exists

- **One API, three deployments.** The local daemon, a hosted daemon and the
  Worker serve the same `/api/*` routes from `crates/clax-server/src/routes/`.
- **Clients, each with its own copy of the API:**
  - the shell, `web/shell/src/api.ts`, same-origin `fetch`, or in the Clax
    app the host transport `web/shell/src/transport.ts`;
  - the extension, `web/extension/src/sw/api.ts`, which imports the shell's
    types, sends `Authorization: Clax-Extension cxe_…` from a pairing made
    through the native host (`web/extension/src/sw/pairing.ts`), and whose
    CSP (`web/extension/manifest.json`) connects only to `localhost:*` and
    `127.0.0.1:*`;
  - the Pi plugin, `plugins/pi/src/clax.ts`, the thirty-two tools in
    TypeScript against the REST API;
  - the CLI, hooks and MCP shim through `DaemonClient`
    (`crates/clax-mcp/src/client.rs`), local bearer or remote mode;
  - the Clax app, Dart, through request frames into `clax-embed`
    (`apps/clax_app/lib/core/`), whose core makes the calls to clax.dev.
- **Credentials** (`Caller`, `crates/clax-http/src/auth.rs`): the daemon
  bearer token; owner and viewer cookies (`crates/clax-http/src/identity.rs`,
  `SameOrigin` in `crates/clax-http/src/viewer.rs`); the extension's
  credential, which is the owner identity
  (`crates/clax-http/src/extension.rs`); GNAP-shaped grants with
  Ed25519-signed requests, `clax-ed25519 v2` (`crates/clax-core/src/remote.rs`,
  `crates/clax-server/src/routes/grants.rs`), whose rights are
  `clax_core::rights::Rights`.
- **Sign-in by code and approval**: `clax remote add` and the app's
  clax.dev Connect (`docs/usage.md`, "Publishing to a public Clax", "Apps
  on clax.dev, from the Clax app").
- **Live pages** (`crates/clax-core/src/live.rs`,
  `crates/clax-store/src/live.rs`, `docs/contract.md` "Live pages"):
  artifacts of kind `live` keyed by `PageKey` (origin and path), versions
  that are snapshots, threads anchored to elements, ranges and areas with a
  PNG clip, sites that join origins, and scope watches
  (`PageKey::covered_by`). On a hosted Clax a live page is the owner's
  alone (`docs/superpowers/specs/2026-10-22-no-god-account-design.md` §3).
- **Thread ownership** (`docs/contract.md` "Thread ownership"): a sent
  thread has one owner agent; another agent's reply, resolve or handoff is
  409 `thread_taken`; ownership ends with the session. `working` records
  expire after `WORKING_TTL_SECS` (120 s, `crates/clax-core/src/working.rs`)
  unless renewed.

## 4. The SDK

### 4.1 Layers

1. **Contract.** Generated from the routes (D2): every public operation's
   method, path, request and response types, error codes, and per
   operation:
   - `retry`: whether it can be sent again after a network error with no
     answer (today the extension writes that rule by hand in
     `web/extension/src/sw/api.ts`);
   - `channel`: the credential kinds it accepts (§5);
   - `rights`: the `Rights` action it needs;
   - `stream`: whether it answers an event stream.

   Routes are marked public or internal on the route itself; the
   generator leaves internal ones out (perf, admin, dev and diagnostic
   routes, the shell's own routes). The contract has a version; CI diffs it
   against the last release and fails a breaking change that does not raise
   the major version.
2. **Client core**, written once per language: request building, the one
   error shape (`{code, message, status}`, as `ApiFailure` has it),
   retries by the contract's `retry` and the clock-skew rule of `Retry`
   (`crates/clax-core/src/remote.rs`), `/api/stream` subscriptions
   multiplexed over one stream, and the modules: artifacts and versions,
   assets, threads and comments, inbox, db, questions, live pages and
   sites, external artifacts (§6), queues (§7), people and grants, and
   agent sessions (in a module of its own: only harness integrations use
   it).
3. **Seams**, chosen per client:
   - `Transport`: `fetch` to a base URL; the app's host channel; `clax-embed`
     frames; in-process (the Worker, tests).
   - `Credential` (§5): browser session; signed grant. A credential owns
     what happens on a 401 (the extension's pairs again once and retries
     once, as today).

### 4.2 Discovery

`GET /.well-known/clax` answers what the instance is, without a
credential: `{kind: "local" | "hosted" | "worker", contract_version,
sign_in: [...], capabilities: [...]}`. `sign_in` names the ways a client
can obtain a grant there (native-host pairing on a local daemon, code and
approval, popup sign-in, reporter tokens); `capabilities` names features
the instance has turned on (queues, git remotes, the db, bots). A client
is given a base URL and adapts to the answer; nothing branches on the
host name.

### 4.3 Languages and packages

- **TypeScript**, `web/sdk`: types generated from the contract, a
  hand-written client core, modules a bundler can drop when unused (the
  shell and the extension have size budgets). The shell, the extension, the
  embeddable overlay (§8) and Pi use it.
- **Rust**, `crates/clax-client`: `DaemonClient` moved out of `clax-mcp`
  onto the contract's types. It sits above `clax-core` in the layering
  `scripts/check-crate-layers.py` defines; `clax-mcp`, `clax-cli` and
  `clax-hooks` depend on it.
- **Dart**: types generated from the contract, over the `clax-embed` frame
  transport in the app.

### 4.4 Each client on the SDK

| Client | Transport | Credential |
|---|---|---|
| Shell, browser | same-origin `fetch` | browser session |
| Shell, in the app | host channel | browser session (the app's) |
| Extension | `fetch` to the paired instance | signed grant |
| Pi, CLI, hooks, shim | `fetch` / `reqwest` | signed grant |
| Clax app | `clax-embed` frames | the core's |
| Deployer's app | `fetch` to their instance | reporter token (§5.3) |

## 5. Credentials

### 5.1 Two kinds

- **Browser session.** The `clax_viewer` cookie, `HttpOnly` and
  `SameSite=Strict`, with the `SameOrigin` check, on the Clax origin only.
  The SDK never holds its secret (`credentials: "same-origin"`). In the
  Clax app the host transport is its twin. **Consent is given only on
  this channel**: approving or widening a grant, answering `ask` and
  `publish_public`, changing sharing, creating queues and service grants.
- **Signed grant.** Everything else: an Ed25519 key, a grant carrying
  `Rights`, requests signed with `clax-ed25519 v2`. In a browser the key is
  a non-extractable WebCrypto key kept in IndexedDB; natively a key file
  (`remote.key`, as today). Clients differ only in how they obtain one:

  | Client | How it gets its grant |
  |---|---|
  | Extension, local daemon | native-host pairing, minting a grant of the owner's row in place of a `cxe_` credential |
  | Extension, hosted or Worker | code and approval |
  | CLI, hooks, shim | `clax connect`, or locally the daemon token, once, to mint a grant |
  | Deployer's app | reporter tokens (§5.3) |

The daemon bearer token becomes a bootstrap: local processes of the
installing user use it to mint a grant, and requests then name a
principal, a grant and its rights.

### 5.2 Types from guards

A route's auth extractor is recorded in the contract (`channel`, `rights`).
The SDK's client type is generic over its credential, so `grants.approve`
exists on a browser-session client and not on a signed-grant one; a
consent call from the extension fails to compile rather than answering
403.

### 5.3 Reporter tokens: no second sign-in (D11)

- **Setup, once.** A person with the right to administer a queue creates a
  *queue service grant* for it, in a browser session. Its one right is to
  mint reporter tokens for that queue. The deployer keeps it in their
  backend's secrets.
- **Per employee session.**
  1. The overlay (§8) creates a non-extractable WebCrypto key.
  2. It calls the SDK's `auth` callback, which the deployer implements
     against their own backend; the backend knows the employee from the
     deployer's own session.
  3. The backend calls `POST /api/queues/<q>/reporter-tokens {reporter_id,
     origin, public_key}`, signed with the service grant.
  4. The answer is a token bound to that key, that origin and that
     `reporter_id`, valid 15 minutes (D12). The SDK calls `auth` again
     before it expires.
- **What a reporter token may do:** file reports into its queue, and read,
  reply on and see the replies to the reports `reporters_see` (§7.2) shows
  that `reporter_id`. Nothing else; it is never a person.
- **`reporter_id`** names one of the deployer's users: every user of their
  app has their own, and it means something only within its queue (the
  same string in another deployer's queue is someone else). Clax makes no
  object per reporter and needs no uniqueness of it beyond that.
- **It is taken from the token**, never from a request body. Clax
  trusts the deployer's backend to name its employees; that is the trust
  boundary. Rate limits apply per `reporter_id` and per service grant;
  revoking the service grant ends every reporter token it minted.
- **Origin.** A reporter token is accepted only with an `Origin` equal to
  the one it is bound to, which is on the queue's list of origins (§7.1);
  the CORS answer names that origin alone.

### 5.4 The extension on a hosted Clax

The extension pairs with each instance it is given (D1): the native host
for the local daemon, code and approval for a hosted Clax or the Worker.
Its CSP `connect-src` and `img-src` widen to the instances it has paired
with; the manifest's `minimum_chrome_version` rises to the version whose
WebCrypto has Ed25519. On a hosted Clax there are no live pages for
anyone but the owner (§3), so there the extension files into a queue whose
origins cover the page (§7.1), or onto the external artifact a page
declares (§6.3); elsewhere it does nothing on that instance.

## 6. External artifacts (D6, D7)

### 6.1 Shape

An artifact of a new kind, `external`, beside `html` and `live`
(`crates/clax-core/src/live.rs` `KIND_*`):

- `external_id`: an opaque string, at most 512 bytes, compared byte for
  byte, never parsed. Clax looks artifacts up by it and returns it on
  every read, so an app maps a Clax artifact back to its own record. It
  is unique per instance, and the same ID from any app, queue or person
  is the same artifact (D17).
- `metadata`: an opaque JSON value, at most 64 KiB, stored and returned as
  written; never interpreted, indexed or searched. Written with
  `if_version`, as db writes are, by anyone who may edit the artifact
  (D15). Readable by anyone who may read the artifact: it holds no
  secrets.
- `revision`: an optional opaque string, the app's revision of its record;
  each thread's anchor records the revision it was made on.
- Title, URL, tags, threads, comments, watches, presence, working records
  and agent feedback, as any artifact has. Clax never stores its content.

### 6.2 Operations

- `POST /api/external {external_id, title, url, metadata?, revision?}`
  finds or creates the artifact (`clax.artifacts.attach` in the SDK).
- `GET /api/external?external_id=` finds one; `PATCH` changes title, URL,
  metadata (with `if_version`) and revision; `DELETE` deletes it with its
  threads.
- Content-only tools and routes (`publish`, `fork`, `git_remote`, versions'
  files) answer 400 `external_artifact`, as they answer `live_page` today.

### 6.3 Pages declare their artifact

A page names its external artifact with
`<meta name="clax-artifact" content="<external_id>">` (read by the
extension and the overlay), or by calling `attach` through the SDK. The
same record seen at several URLs is then one artifact with one set of
threads. A page that declares nothing stays a URL-keyed live page on a
local daemon, as today.

### 6.4 Threads carry an opaque reference too

Each thread may carry `reporter_id` (from a reporter token, §5.3) and
`metadata` (an opaque JSON value, at most 16 KiB, written with the thread
and changed with `if_version`): the app's version, account, environment,
flags, a tracker link an agent wrote. Like an artifact's, Clax returns it
and never reads it.

### 6.5 Agents

`read`, `comments_read` and feedback deliveries include `external_id`,
`metadata`, `revision` and `reporter_id`. `docs/contract.md` says that
metadata, like comment text, is untrusted input written by the app's
users, and agents treat it as data. An agent sees the record itself at its
URL: Clax has no copy.

### 6.6 Audit (D15)

Every change to an external artifact and to a thread's metadata is an
event in the Toolpath journal (`audit_events`, `docs/contract.md`
"Toolpath journal and export"), written in the same transaction as the
change, with the actor, `via` and `call` the envelope already carries.
New kinds:

| Kind | Body |
|---|---|
| `external.create` | `external_id, title, url, revision, metadata, metadata_sha256` |
| `external.update` | the fields changed, each with its new value; for `metadata` the new value, `metadata_sha256` and `previous_sha256` |
| `external.delete` | `external_id, metadata_sha256` |
| `thread.metadata` | `thread_id, metadata, metadata_sha256, previous_sha256` |

The journal keeps each metadata value, so its history can be rebuilt
from the events alone; the 64 KiB and 16 KiB caps bound each event. A
write that changes nothing records nothing, as for every kind.

## 7. Queues

### 7.1 A queue

A queue is an object of its own, made by a person in a browser session:

- `name`, and a `scope` of what it collects: the origins of the deployer's
  app (a site's origins, as "Joined sites" in `docs/contract.md` groups
  them), external IDs with a given prefix, and tags;
- `reporters_see`: `own` (the default, D8) or `all`;
- its members: people and agents with the **triage** right (read every
  report, claim, reply, resolve, write thread metadata) and the
  **administer** right (change the queue, create service grants);
- the artifacts that hold its reports: external artifacts (§6) and, for
  pages that declare none, a URL-keyed artifact per page that belongs to
  the queue, not to the owner, so that it is in reach on a hosted Clax
  (`2026-10-22-no-god-account-design.md` §3) for the queue's members.

One external artifact can hold reports of several queues (D17). Each
thread records the queue it was filed through, and a queue's rights reach
its own threads: reporters and triage members of queue A never see queue
B's reports on the same artifact. The artifact's own fields (title, URL,
`metadata`, `revision`) are shared: whoever reaches the artifact through
any queue reads them, and its editors through any queue write them.

### 7.2 Visibility

With `reporters_see: own` a reporter token reads only threads whose
`reporter_id` is its own; with `all`, every report in the queue. Triage
members read every report. On a hosted Clax the reach rule applies to
members as to anyone: the triage right is held at the queue's artifacts,
as a share is.

### 7.3 The queue's listing

`GET /api/queues/<q>/reports?state=open|claimed|resolved&tag=&cursor=`
lists reports in order of arrival, with a cursor. An agent pulling work
calls it; `wait_for_feedback` and scope watches still wake an agent when a
report arrives.

### 7.4 Claims are leases (D10)

- `POST /api/queues/<q>/reports/<tid>/claim` answers `{lease_id,
  expires_at, fence}`, or 409 `thread_taken` naming the holder. A claim
  makes the claimant the thread's owner (`docs/contract.md` "Thread
  ownership").
- The lease lasts 10 minutes. `renew` extends it; `working` on a claimed
  report renews it too (agents call `working` every
  `WORKING_TTL_SECS` while they work), and shows the reporter "in
  progress".
- `release` puts the report back; so does expiry, and the end of the
  claimant's session.
- Every write a claimant makes as claimant (reply, resolve, metadata)
  carries the `fence`. A write with an older fence than the report's
  current one is 409 `lease_lost`, so an agent whose lease lapsed cannot
  act on a report another agent now holds.
- Expiry runs on the injected clock (`AGENTS.md`); no test waits for it.

### 7.5 Replies reach reporters

A reporter is not a Clax person and has no inbox. The overlay's "your
reports" view shows a reporter's reports and their replies through the
reporter token, and outbound webhooks (§9) let the deployer tell employees
through their own channels.

## 8. The embeddable overlay

### 8.1 What it is

The selector the extension's content script runs today (comment mode,
element, range and area picks, the composer; `web/bridge`), shipped as a
script for a deployer's own pages, on the SDK:

```ts
Clax.overlay({
  instance: "https://clax.corp.example",
  queue: "acme-app",
  auth: () => fetch("/clax-token", { method: "POST", body: publicKey }),
  metadata: () => ({ version, account, flags }),
});
```

It shares `web/bridge` with the extension, so the two never drift.

### 8.2 Clips (D9)

- **In the overlay:** the clip is rendered from the page's DOM. What it
  cannot render (cross-origin images without CORS, `<canvas>`, `<video>`,
  cross-origin iframes, some CSS effects) is listed with the clip, so a
  triager knows the image is partial.
- **With the extension installed:** its `captureVisibleTab` capture is used
  instead.
- **Exact screenshot, optional:** a deployer may offer `getDisplayMedia`
  behind an explicit "send exact screenshot" action; the browser asks each
  time, so it is never the default.

## 9. Outbound webhooks

A queue may name webhook endpoints. Clax posts `report.created`,
`report.claimed`, `report.replied`, `report.resolved` and
`report.released`, each with the report's IDs, `external_id`,
`reporter_id`, state and a link, signed with a secret of the endpoint's;
delivery is at least once, with retries and backoff, and each event
carries an ID the receiver uses to drop repeats.

## 10. Order of work

1. The contract generated from the routes, public and internal marked, and
   the TypeScript and Rust SDK cores; the shell moves onto `web/sdk` with no
   change in behaviour.
2. Signed grants for the extension (native-host pairing, then code and
   approval), and the extension on the SDK (D1).
3. Discovery (`/.well-known/clax`).
4. External artifacts and thread metadata.
5. Queues: scope, rights, `reporters_see`, the listing, leases.
6. Queue service grants and reporter tokens.
7. The embeddable overlay with DOM clips.
8. Webhooks.
9. Pi, the CLI, hooks and the shim onto grants and the SDK; the daemon
   token becomes a bootstrap; `Caller` folds the owner and owner-agent
   token callers and the extension's credential into grants.

## 11. Non-goals

- Identity a deployer defines (OIDC, token exchange, SCIM) (D5).
- Packaging and running Clax in Kubernetes (D4); the store is SQLite with
  one writer, so a deployment is one replica with a persistent volume.
- Licensing (D3).
- Searching or indexing metadata.
- Turning live pages into external artifacts of a built-in source.

## 12. Constraints

- `docs/contract.md` changes with the code: credentials, the extension,
  external artifacts, queues, leases, reporter tokens, webhooks, and the
  errors `external_artifact` and `lease_lost`, and the
  Toolpath "Kinds" table (§6.6).
- Migrations take the next free number in
  `crates/clax-store/src/migrations.rs`.
- Every SDK transport passes one conformance suite against the real
  routes (daemon, Worker, `clax-embed`); the SDK ships one fake that
  refuses what the real routes refuse, which replaces the clients' own
  fakes (`web/extension/test/fake-chrome.ts` for its API calls,
  `apps/clax_app/lib/remote/fake_remote_api.dart`, Pi's).
- Rights tests for reporter tokens: another queue, another origin,
  another `reporter_id` under `own`, a revoked service grant, and every
  route outside the queue; and two queues filing onto one external
  artifact, where neither queue's reporters or triage members read the
  other's threads.
