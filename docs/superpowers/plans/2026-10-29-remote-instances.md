# Remote instances Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** an agent given any Clax artifact URL (local, `just clax-dev-local`, clax.dev, on-prem) reads, watches and answers its comments as fluidly as a local one. A local process that can only reach the daemon's port gains none of the owner's powers.

**Architecture:**
- **Security first:** close every hole in Task 0's inventory.
- **Per instance:**
  - Discovery describes each instance.
  - A 404 hint is the same for every artifact ID.
- **On the local daemon:**
  - One resolver decides what a URL names.
  - Agents can file connection requests; the person approves them.
  - A proxy reads a remote artifact and its comments with the stored token.
  - Thread mirrors carry remote comments through every local delivery route.

**Tech Stack:** Rust (axum, rusqlite, reqwest), Bazel, the Svelte shell, vitest.

**Spec:** `docs/superpowers/specs/2026-10-29-remote-instances-design.md`

## Global Constraints

- Build and test through Bazel (`just test-affected`, then `just check`); never `cargo test` in the worktree.
- Crate layering per `scripts/check-crate-layers.py`:
  - `clax-mcp` reaches remotes only through the local daemon's routes.
  - Remote HTTP calls live in `clax-server`, on `public::Client`.
- Tests are fast: no sleeps, injected clocks (`clax_testkit::clock`), helpers from `clax-testkit` only. Use `clax_testkit::remotes::Public` as the remote and `clax_testkit::remotes::connect` to connect.
- No `unsafe`, no hard links, no Perl.
- Migrations: one new migration, the next free number (60), in `crates/clax-store/src/migrations.rs`.
- Commits: conventional subjects, no attribution lines, `git -c commit.gpgsign=false commit` when signing fails.
- Before editing a file, run `qualifier threads <path>`. Record each option not taken as an `alternative` with a `revisit:` tag.
- "ID" in prose. Doc comments state the contract.
- HTTPS for remotes except on loopback (`clax_core::config::base_url_ok`).
- Remote use (resolver remote answers, proxy, requests): only `Caller::Owner` and `Caller::OwnerAgent`, as `routes/remotes.rs` `asker` does.

## Review Focus

1. **A remote that is down or slow.** read, comments and watch must answer within the tool's usual timeout with a clear "<name> is unreachable" error. The local daemon must never hang. Test in Task 5: a `Public` that has been stopped.
2. **A token revoked on the remote.** On a 401, the remote is marked signed out. The next call answers `sign_in_required` with a connection request, not a raw 401. Test in Task 5.
3. **The same remote URL with a trailing slash, `/get`, `?v=` or a fragment.** All forms resolve to the same artifact. Test in Task 3's resolver table.
4. **Unwatching while a remote comment is in flight.** No notice reaches an ended session, and no orphan remote session is left behind. Test in Task 6.
5. **Two agents asking for the same origin at the same moment.** Exactly one request and one card. Test in Task 4, as concurrent POSTs.

---

### Task 0: Close the local-trust holes

Owner's decision: qualifier `c3cd21b0`, `status:decided`.

**The rule:** reaching the daemon's port gives nothing beyond the open paths. Owner power needs either:
- the token in `<home>/daemon.json` (0600, home 0700), or
- an owner browser session that a token holder opened for a browser.

No owner credential may be derived from, or handed out against, anything a local process can forge (`Sec-Fetch-Site`, `Origin`, `Host`, `X-Clax-Channel`).

**Holes closed** (inventory IDs A1–A10, from a read of the code; file:line below):
- **A1.** `GET /api/token` (`crates/clax-server/src/routes/token.rs:28-47`) answers only a request carrying a valid owner browser session cookie. For any other caller it returns 401, whatever its peer or headers.
- **A2.**
  - The owner cookie and the events cookie are random session secrets, kept hashed in a new `browser_sessions` table (migration 60, the same migration as Task 4's tables).
  - They are no longer SHA-256 of the token (`crates/clax-http/src/identity.rs:60-67`, `crates/clax-http/src/auth.rs:222-230`). Each is revocable, and they survive daemon restarts.
  - `Sec-Fetch-Site` is no longer what mints them.
- **A4.** On a local daemon, a caller with no credential reaches only:
  - `open_without_credential` paths
  - the login route
  - the cold-browser page
  - the invite and connect flows (`/connect/*`, `/join/*`, `/api/grants*` as today)
  - site-host content whose access lets anyone in

  It cannot mint a `clax_viewer` (`routes/viewers.rs:76-90`), comment, or send a thread to an agent. A connected person's cookie, and grants, work as today.
- **A5.**
  - The claim of a `clax_viewer` for the owner moves from `/api/token` to the login route, so only a browser redeeming a code the token holder made claims anything.
  - The merge path forgets the merged viewer's cookies, as the adopt path does (`crates/clax-store/src/viewers.rs:850-910`).
- **A6.** A peer on the machine's own LAN address is held to the same rule as loopback. `browser_url` on a LAN bind opens through a login link, so the owner's browser is the owner there too.
- **A7, A8, A9.** These were only as strong as the token, so they are fixed by A1 and A2. Tests prove each refuses a forged-header caller with no token.

**How a browser becomes the owner:**
- `POST /api/browser-login {next}` (`RequireToken`) makes a one-use code: 32 random bytes, valid for 2 minutes, stored hashed.
- `GET /login?code=<c>&next=<path>` redeems it. It sets the owner and events session cookies (HttpOnly, `SameSite=Lax`, about 400 days), claims a single viewer cookie, and redirects 303 to `next`, which must be a local path.
- Every opener passes a code:
  - `clax open` (`crates/clax-commands/src/commands/open.rs`)
  - the MCP `open` tool (`crates/clax-mcp/src/tools.rs:3293`)
  - `clax serve`'s "open the gallery"
  - `just watch`/`just dev`'s printed and opened URLs
  - the extension's "Open in Clax" (through its native host, which holds the token)
- `clax login [path]` prints a fresh login link, for a browser on its own.
- A browser that opens `http://localhost:7480/…` with no session gets a small page (local daemons only): "Open Clax from your terminal: `clax open` (or `clax login` for a link)."

**Ergonomics:**
- A browser already holding a session never notices.
- A cold browser profile needs one `clax open`. That's the one new step, and it happens once per profile.
- The CLI, MCP, hooks, the Pi plugin, the native host and the in-process app read `daemon.json` or run in process, and are unchanged.

**Not closable in the daemon (A3):** an agent running as the owner's OS user can read `daemon.json`, as any process of that user can, and so can pose as the owner (`CLAX_CHANNEL=owner`). The daemon cannot tell such an agent from the owner's own CLI. Task 0 does the following:
- **Sessions mark agents.** Hooks and the MCP shim send `X-Clax-Session`, which the daemon already counts as `OwnerAgent`, so an agent that has not tampered with its client is always the agent.
- **Browser-only decisions need a browser session cookie, not the bearer token.** These are the M20 decisions:
  - `grants::approve` and `require_browser` (`grants.rs:41-50, 864-874`)
  - setting rights and revoking people (`people.rs:398, 696, 738`)
  - answering a remote connection request (Task 4)

  An agent wanting one must open a browser session, which it can only do by driving the login route itself.
- **A waiver records the residue.** On `crates/clax-http/src/identity.rs`, it says only OS-level sandboxing of agents closes A3, and is tagged `status:needs-decision` for the owner: whether to also require a confirmation in the owner's browser for every owner-level write made with the bearer token while an agent session is live.

**Tests** (new file `crates/clax-server/tests/api_local_trust.rs`; daemons from `TestServer::spawn`, which must stop setting `require_credential` so the default path is tested):
- `the_token_is_refused_to_a_loopback_caller_without_a_session`. Plain, with `Sec-Fetch-Site: same-origin`, and with `Origin` set to the daemon's: 401 each time.
- `a_login_code_opens_one_browser_session_once`. Redeeming it sets the cookies. A second redemption gets 410. A code past 2 minutes gets 410 (`ManualClock`). A `next` that is not a local path gets 400.
- `owner_cookies_are_not_derived_from_the_token`. A cookie computed as SHA-256 of the token under the old label is refused.
- `a_caller_without_a_credential_cannot_comment_or_mint_a_viewer`
- `a_planted_viewer_is_claimed_only_at_login` (and its cookie is forgotten after a merge)
- `browser_only_decisions_refuse_the_bearer_token`
- `a_lan_address_peer_is_held_to_the_loopback_rule`
- `the_extension_gateway_refuses_a_forged_origin_without_a_credential`
- `a_cold_browser_gets_the_open_from_your_terminal_page`
- Shell (vitest): boot with a session cookie works. Boot without one shows the cold-browser page.

**Commits:**
- `fix(auth): serve the daemon token only to an owner browser session`
- `feat(cli): open the browser through a one-use login link`
- `fix(auth): refuse credential-less writes on a local daemon`

### Task 1: Discovery describes the instance

**Files:**
- Modify: `crates/clax-server/src/routes/discovery.rs`
- Modify: `crates/clax-core/src/config.rs` (add the optional hosted `name`)
- Modify: `crates/clax-worker/src/instance.rs` (read the `INSTANCE_NAME` var)
- Test: `crates/clax-server/tests/api_discovery.rs` (new; add to `crates/clax-server/BUILD.bazel` the way the other test shards list theirs)

**Produces:** `/.well-known/clax` returns `{kind, contract_version, name, origin, api: "/api", site_domain?, sign_in:[…,"device"], device:{code:"/api/hosted/device/code", approve:"/device", token:"/api/hosted/device/token"}?, capabilities}`. `device` and `"device"` appear only on hosted instances.

- [ ] Write `discovery_names_a_hosted_instance_and_its_device_sign_in`. It asserts:
  - `name` is the configured name, or the host when no name is set
  - `origin == ts.base`
  - `api == "/api"`
  - `sign_in` contains `"device"`
  - the three `device` paths
  - `site_domain` equals the configured site domain
- [ ] Write `discovery_of_a_local_daemon_offers_no_device_sign_in`. It asserts `kind == "local"`, and that there is no `device` and no `site_domain`.
- [ ] Implement. Fix the module doc's claim of a `worker` kind (the Worker answers `hosted`). Document `INSTANCE_NAME` in `crates/clax-worker/README.md` and `docs/contract.md`'s discovery section.
- [ ] Commit `feat(discovery): describe the instance, its site domain and device sign-in`.

### Task 2: One hint on every 404 of an artifact

**Files:**
- Modify: `crates/clax-http/src/error.rs` (`ApiError::not_found_artifact(origin: &str) -> ApiError`)
- Modify: `crates/clax-http/src/hosted.rs` (`guest_may` and the other hosted not-found paths under `/a/`)
- Modify: `crates/clax-server/src/shell/` (HTML for `Accept: text/html`, reusing `get_page.rs` `unavailable()`'s chrome)
- Test: `crates/clax-server/tests/hosted_pages.rs`

**Produces:** on a hosted instance, every 404 of `/a/<id>` and its subpaths has `error.code = "not_found"`, `message = "not found"` and `hint`, exactly:

> This may be a private page. If it is yours, sign in at <origin>/signin. Agents: connect this Clax instance with the connect_instance tool or `clax remote request <url>`.

It also carries `sign_in = "<origin>/signin"` and `instance = "<origin>/.well-known/clax"`. For `Accept: text/html`, the same words come as a page with a Sign in button. Local daemons are unchanged.

- [ ] Write `a_missing_and_a_private_artifact_404_alike`. For signed-out and signed-in callers, and for JSON and HTML, the body for a never-created ID equals the body for another person's private artifact, byte for byte. Assert the JSON `hint` text above.
- [ ] Implement, then run `//crates/clax-server:integration__*` for `hosted_pages`.
- [ ] Commit `feat(hosted): hint at sign-in and connecting on every artifact 404`.

### Task 3: One resolver for every URL

**Files:**
- Create: `crates/clax-server/src/resolve.rs`. It holds:
  - `pub async fn resolve(s: &AppState, url: &str) -> Result<Resolved, ApiError>`
  - `pub struct DiscoveryCache` (in memory, on `AppState`, taking the injected clock)
- Create: `crates/clax-server/src/routes/resolve.rs` (`GET /api/resolve?url=`)
- Modify: `crates/clax-server/src/router.rs`
- Modify: `crates/clax-mcp/src/target.rs`:
  - add `Target::Remote { remote: String, origin: String, artifact_id: String, version: Option<u32>, url: String }`
  - add `Target::SignInRequired { origin: String, name: String }`
- Modify: `crates/clax-mcp/src/tools.rs` (`target()`: a URL not on the daemon's base goes to `client.resolve(url)`)
- Modify: `crates/clax-mcp/src/client.rs` (`pub async fn resolve(&self, url: &str) -> Result<Value>`)
- Test: `crates/clax-server/tests/api_resolve.rs` (new) and `crates/clax-mcp/src/target.rs` unit tests

**Produces:** `Resolved` serialises as one of:
- `{"kind":"local","artifact_id","version"?}`
- `{"kind":"remote","remote","origin","name","artifact_id","version"?,"url"}`
- `{"kind":"sign_in_required","origin","name"}`
- `{"kind":"page","url"}`

The order is:
1. The URL is a local artifact's own.
2. The URL is the public copy of a pushed artifact, found in `remote_links` by URL or by remote and remote artifact ID. This resolves to `local`.
3. The URL is on a connected remote's origin or its `site_domain`. This resolves to `remote`.
4. Discovery says it is a Clax instance. This resolves to `sign_in_required`.
5. Otherwise, `page`.

Discovery answers are cached: negative ones for 10 minutes on the injected clock, positive ones until a call fails. The URL forms are `<origin>/a/<id>[/…][?…][#…]`, `<origin>/c/<id>/v/<n>/…` and `<id>.<site_domain>`. A caller other than the owner or an owner agent gets only `local` or `page`.

- [ ] Write `resolve_tells_local_remote_unconnected_and_page_apart`, a table over:
  - a local ID URL
  - `Public`'s `/a/<id>`, with `/get`, a trailing `/`, `?v=2` and `#t`
  - a site-domain URL
  - a second, unconnected `Public`
  - `http://127.0.0.1:<free port>/x` (no Clax there)
  - a pushed artifact's public URL, which must resolve to `local`
- [ ] Write `a_connected_persons_agent_resolves_remote_urls_as_pages`.
- [ ] Write `a_negative_discovery_is_kept_ten_minutes` using `ManualClock`.
- [ ] Implement. `do_watch`, `resolve_ref` and `resolve_id` match the new variants:
  - **`SignInRequired`:** render the error `remote_sign_in_required`, with `{origin, name}` and a `next` line naming `connect_instance`.
  - **`Remote`:** handled by Tasks 5 and 6. Until those land, answer `remote_not_yet`.
- [ ] Commit `feat(resolve): tell local, remote, unconnected and page URLs apart in one place`.

### Task 4: Connection requests

**Files:**
- Modify: `crates/clax-store/src/migrations.rs`, migration 60. It adds:
  - `remote_requests (id TEXT PK, origin TEXT NOT NULL, name TEXT NOT NULL, status TEXT NOT NULL CHECK (status IN ('open','connected','dismissed','muted','expired')), created_at, answered_at, remote_id)`
  - `remote_request_askers (request_id, session_id, url, reason, asked_at, PRIMARY KEY (request_id, session_id))`
  - `remote_mutes (origin TEXT PK, muted_at)`
  - `remote_dismissals (origin TEXT PK, until TEXT NOT NULL)`
  - Task 6's columns (`artifacts.kind` value `remote_mirror`; `remote_links.followed INTEGER NOT NULL DEFAULT 0`)
- Create: `crates/clax-store/src/remote_requests.rs`:
  - `ask_remote(&self, origin, name, session, url, reason, now) -> Result<Asked>`
  - `pub enum Asked { Opened(Request), Joined(Request), Connected(String), Muted, Backoff { until }, Capped }`
  - `answer_remote_request(id, Answer)`
  - `remote_request(id)`
  - `open_remote_requests()`
  - `mute_origin` and `unmute_origin`
- Create: `crates/clax-server/src/routes/remote_requests.rs`. Agents may call:
  - `POST /api/remotes/requests {url, reason?}` (needs a session)
  - `GET /api/remotes/requests/<id>?wait=<s>` (long-poll, ≤50 s, like `wait_for_feedback`)

  Only the owner's browser (`SameOrigin`) may call `POST /api/remotes/requests/<id>/answer {answer: connect|not_now|never}`. `connect` runs `routes/remotes.rs`'s add path and returns its `verification_url` for the shell to open.
- Modify: the shell (`web/shell/src`). It shows the card in the inbox; its copy is "<agent> wants to connect to <name> (<origin>) to read <url>", with the buttons Connect, Not now and Never for this instance. Settings → Remotes lists and unmutes mutes.
- Modify: `crates/clax-mcp/src/tools.rs` (tool `connect_instance {url_or_origin, reason?}`, which posts and waits)
- Modify: `crates/clax-commands/src/commands/remotes.rs` (`clax remote request <url>`; the `not_for_agents` line on `add` gains "agents: use `clax remote request <url>`")
- Test: `crates/clax-server/tests/api_remote_requests.rs` (new), `crates/clax-store` unit tests, `web/shell` vitest for the card

**Rules (spec §3):**
- Asking without a session gets 403 `session_required`.
- An origin that is not a Clax instance gets 422 `not_a_clax`.
- An origin already connected answers `{status:"connected", remote}`.
- A muted origin answers `{status:"muted"}`.
- Within 24 h of a "Not now", an ask answers `{status:"not_now", until}`.
- One open request per origin. Other askers join it.
- At most 3 new requests per session in 24 h, then 429 `too_many_requests`.
- An open request expires after 24 h.
- The person hears once, when the request opens, through the inbox and the asking agent's thread (`where_to_ask`'s placement).
- A denied or expired device flow answers waiting agents `dismissed`.

- [ ] Write the store tests:
  - `asks_fold_into_one_open_request_per_origin`
  - `not_now_holds_an_origin_off_for_a_day`
  - `never_mutes_until_unmuted`
  - `a_session_opens_at_most_three_requests_a_day`
  - `an_open_request_expires_after_a_day` (on `ManualClock`)
- [ ] Write the route tests:
  - `an_agent_without_a_session_cannot_ask`
  - `two_agents_asking_at_once_make_one_card`
  - `connect_runs_the_device_flow_and_wakes_the_waiting_agent` (approved via `Public::browser`)
  - `a_denied_device_flow_answers_dismissed`
  - `an_expired_device_flow_answers_dismissed`
  - `answering_is_the_owners_browser_alone`
- [ ] Implement store, routes, the MCP tool, the CLI and the card. Run `just test-affected`.
- [ ] Record the qualifier `alternative` "Letting agents run `add` directly" on `routes/remote_requests.rs`, with `revisit:requests prove too slow in practice`.
- [ ] Commit `feat(remotes): let agents ask to connect an instance, the person approving once`.

### Task 5: Read and comment on a remote artifact

**Files:**
- Create: `crates/clax-server/src/routes/remote_artifacts.rs`. It serves `GET /api/remotes/<rid>/artifacts/<aid>` and `…/versions/<n>/files/*path`. It also proxies `GET|POST …/threads`, `…/threads/<tid>/comments`, `…/resolve` and `…/reopen`. It calls the remote with `public::Client` and the stored token. An `Idempotency-Key` from the caller is passed through.
- Modify: `crates/clax-mcp/src/tools.rs`. `read`, `comments_read`, `comments_reply`, `comments_resolve` and `comments_handoff` on `Target::Remote` call those routes through `client` and render as for local, adding `remote: {origin, name}` and `url`. Tools in spec §4's "not offered" list answer `not_on_remote` and name the instance.
- Test: `crates/clax-server/tests/api_remote_artifacts.rs` (new), `crates/clax-mcp` tests where read rendering is tested

**Produces:**
- **Withheld comments.** When the remote withholds comments from agents, read and comments_read say "<n> comments from people you have not approved for agents on <name>" (`withheld` in the remote's thread answer, the field `crates/clax-http/src/agent.rs` filters by).
- **Unreachable remote.** 502 `remote_unreachable`, `{origin, name}`.
- **Revoked token.** A 401 from the remote marks the remote `signed_out`, then answers `remote_sign_in_required`.

- [ ] Write `reads_a_private_remote_artifact_with_the_stored_token`.
- [ ] Write `replies_and_resolves_on_a_remote_thread_once_per_key` (the same `Idempotency-Key` twice makes one comment).
- [ ] Write `an_unreachable_remote_answers_remote_unreachable`.
- [ ] Write `a_revoked_token_asks_to_connect_again`.
- [ ] Write `withheld_comments_are_counted`.
- [ ] Implement, then run `just test-affected`.
- [ ] Commit `feat(remotes): read and answer comments on a connected instance's artifacts`.

### Task 6: Watch a remote artifact through a thread mirror

**Files:**
- Modify: `crates/clax-store/src/remotes.rs`:
  - `follow_remote(rid, remote_aid, url, title) -> Result<String>` creates or reuses the mirror artifact (`kind='remote_mirror'`, `hidden=1`, no versions) and its `remote_links` row with `followed=1`, `pushed_n=0`, `remote_n=0`.
  - `unfollow_remote(mirror_id)`.
  - `mirrors_to_drop(now)` returns mirrors no session has watched for 7 days.
- Modify: `crates/clax-server/src/linked.rs`. A followed link opens one remote session per watching local session (named as the local agent, ended when it ends). What arrives for a remote session is routed to the local session's watch of the mirror. Anchors are taken as given (no push records), and replies on a mirror go up in the replying session's remote session.
- Modify: `crates/clax-mcp/src/tools.rs`:
  - `watch` and unwatch on `Target::Remote` call `PUT`/`DELETE /api/sessions/<sid>/remote-watches {remote, artifact_id, url}` (new route in `routes/watches.rs`), which follows the artifact and watches the mirror.
  - Notices for a mirror render the remote's URL and thread IDs.
  - Every tool taking a mirror's thread ID routes to Task 5's proxy.
- Modify: the lists, gallery, search and inbox queries in `clax-store`, which exclude `kind='remote_mirror'`.
- Test: `crates/clax-server/tests/api_remote_watch.rs` (new)

- [ ] Write `a_comment_on_a_watched_remote_artifact_reaches_feedback`. A `Public` person comments, sends it to the agent, and the local session's `/notices` gets it with the remote URL.
- [ ] Write `each_watching_session_is_its_own_agent_there`. Two local sessions watch. A send to one on the remote reaches only that one.
- [ ] Write `unwatching_ends_the_remote_session_and_drops_late_notices`.
- [ ] Write `mirrors_are_hidden_from_lists_and_dropped_after_a_week` (on `ManualClock`).
- [ ] Write `a_pushed_artifacts_public_url_is_never_followed`.
- [ ] Implement. Settle concern `7c8c51dd` for followed links by replying with the test's name.
- [ ] Commit `feat(remotes): watch a connected instance's artifact, its comments arriving like local ones`.

### Task 7: End to end against the dev Worker, and docs

**Files:**
- Create: `demo/remote-e2e.ts` (or a `web/e2e` spec, whichever already starts `clax-dev-local`'s Worker; `demo/lib/worker.ts` `startWorker`)
- Modify: `docs/contract.md` (resolve, requests, remote artifact routes, discovery fields, the 404 hint)
- Modify: `docs/usage.md` (connecting an instance from an agent)

The end-to-end test:
1. Start the Worker with clax-dev-local's settings on a free port, and a scratch `CLAX_HOME` daemon on another free port.
2. The agent calls `connect_instance`. The script answers the card and approves on the Worker through a signed-in person's session (`demo/lib` helpers).
3. `read` a private page.
4. A comment posted on the Worker by its person reaches `clax feedback follow --once`.

- [ ] Write the script. Run it. Expected: each step prints, and it exits 0.
- [ ] Update the docs.
- [ ] Commit `test(remotes): connect, read and follow a private page on the dev Worker end to end`.
