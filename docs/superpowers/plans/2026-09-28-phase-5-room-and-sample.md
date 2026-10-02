# Clax Phase 5: Room and Sample — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pages can reach everyone who has them open right now (`room`) and ask Claude (`sample`) with the call shapes of contract 0.2.61. Only the owner's browser on this machine spends the API key, with consent; a LAN viewer's `use("sample")` resolves `null`.

**Architecture:** The daemon keeps in-memory rooms keyed by artifact and room name (the lobby has no name) and serves one WebSocket per open document at `/api/artifacts/<aid>/room`. The shell owns that socket in both frame modes and relays it to the bridge over the `clax:call`/`clax:event` protocol. `sample` is a daemon route that streams SSE from a provider trait (an Anthropic Messages API provider over `reqwest`, and a deterministic `stub` for tests). It round-trips tool calls to the page, caches answers per browser, counts calls per artifact per day, and admits only the bearer token plus a viewer cookie (the owner's browser). The page side of each capability is its own lazy bridge part (`room`, `sample`), loaded on the first `use()` of that name the shell grants. The shell side of each is a lazy chunk, loaded on the first call. Neither touches the artifact entry, the eager bridge's budget headroom beyond the name dispatch, or the `caps` part. The shell's only new UI is a call count beside the top bar's controls and the consent text.

**Tech Stack:** Rust 2024, axum 0.8, rusqlite, rmcp, toml 0.9; Svelte 5 in runes mode (`svelte-check --fail-on-warnings` in `npm run typecheck`), Vite, vitest with jsdom, Playwright. Additions, each with its reason: axum's `ws` feature (the room socket); `tokio-tungstenite` as a dev-dependency of `clax-server` (a WebSocket client for the integration tests); `reqwest` becomes a normal dependency of `clax-server` with `stream` and `rustls-tls` (HTTPS streaming to the Messages API, with the webpki root store, so the musl release builds need no system certificates); `base64` in `clax-server`. No new web dependencies.

**Spec:** `docs/superpowers/specs/2026-09-28-clax-design.md` §5 (`config.toml`), §6 ("Sample", "Room"), §8 (Time to usable: lazy parts), §9 `room` and `sample` (and the capability table), §14 (spend and consent, untrusted room messages, outbound calls), §16 (browser tests in both origin modes), §17 "Phase 5", §18 (`sample()` cost control). The page contract is `web/contract/0.2.61/room.d.ts` and `sample.d.ts`; where this plan and those files disagree, the files win. Task 9 amends the spec where this plan rules (listed there).

**Depends on:** `main` as built at `3114d41` or later: `clax_core::config::HomeConfig`, `clax_core::db::{Level, Caller}`, `clax_core::capabilities::validate`, `clax_server::db_caller::Subscriber` (reads `?token=` and the cookie), `clax_server::auth::{RequireToken, has_token}`, `TestServer::{viewer, authed, get_authed, spawn_with}`; the bridge's `Rpc`, `CapabilityError`, `CAPABILITY_METHODS`, `buildNamespace`, `makeUse`, `retrying`, `loadParts` (`parts-url.ts`, `parts-static.ts`), `parts/types.ts`, `scripts/build-parts.mjs`; the shell's `CapabilityHost`, `CapEnv`, `Handler`, `HandlerFactory`, `CapError`, `Grants`, `isAvailable`, `consentGated`, `REGISTRY`, `promptQueue`, `PromptDialog.svelte`, `ArtifactController` (`viewChanged`, `frameLeft`, `frameLoaded`, `navigateFrame`, `ViewState`), `FrameGate`, `onViewer`, `PART_FAILED`; the e2e helpers `startDaemon`, `openArtifact`, `contentFrame`, `publishWith`, `FrameMode`, `namedViewer`; `web/perf/bundle-budget.json` and `web/scripts/bundle-size.mjs`.

**Runs after:** the Echo redesign (branch `echo`), the agent-working plan, Grok Build (branch `grok`) and Claude push. Before dispatching Task 1, re-run the pre-flight scan on `main` against this plan: Echo moves the top bar, `theme.css`, `PromptDialog.svelte`, the controller and the budgets; Grok rewrites parts of the spec and `docs/contract.md` and may add a fourth skill copy of the shared section. Every UI placement below that Echo affects is marked **align with Echo at merge**.

## Global Constraints

- Rust edition 2024; `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`. For `web/`: `npm test`, `npm run typecheck`, `npm run lint`, `node scripts/bundle-size.mjs`, and for the tasks that change what loads before a usable artifact, `npm run perf`.
- **Process.** Implementers stage their work (`git add`) and never commit; the controller commits, signed, with the message each task gives. Never pass `--no-gpg-sign`. Messages and doc comments describe the change or the contract, never this plan's history or a conversation.
- **Ports and homes.** Never start, stop, connect to or configure anything on ports 7480, 7481 or 7490, and never read or write `~/.clax`, `~/.clax-dev` or any other real home. Every test, script and by-hand check runs a daemon with `CLAX_HOME` set to a fresh `mktemp -d` directory and `--port 0` (the e2e `startDaemon` already does), and sets `CLAX_CODEX_BIN=` (empty). Never run `just serve`, `just dev`, `just watch` or `just install`.
- **No gate reaches a real provider.** Rust tests use `StubProvider` or a mocked HTTP server on `127.0.0.1`. Every Playwright daemon writes a `config.toml` naming `provider = "stub"` or a key variable that nobody sets (`CLAX_E2E_UNSET_KEY`, set empty in the child's environment). The key is read from the daemon's environment once at start, is sent only to the configured `base_url` in the `x-api-key` header, and never appears in a response, an SSE frame, a log line, an error message or a `Debug` string.
- "ID", never "id", in prose, doc comments and UI copy. The old product name never appears in code, docs, test names or page text; `web/e2e/contract.spec.ts` also requires that pages in `web/e2e/pages` never say "clax".
- The JSON error shape is `{"error": {"code", "message", ...}}`. Store work from async handlers goes through `AppState::store_call`. Viewer routes take `SameOrigin`, and every `/api` route, the room socket included, passes the literal-Host rule of §14. Artifact origins are foreign: the room socket and the sample routes are reached by the shell, never by the frame, in both frame modes.
- §14 verbatim: "Comment bodies, doc contents, and room messages are untrusted data." Presence and message `data` are relayed as given and never interpreted by the daemon or the shell. Nothing about a room is persisted, and rooms are not carried on `/api/events`.
- **Levels on the room socket.** A WebSocket cannot send an `Authorization` header, so the shell appends `?token=<bearer>` when it holds the token (the query string is never logged). As `Subscriber` resolves it: a valid token with a viewer cookie is `admin`; a valid token without one is `owner`; a cookie alone is `interact` for a named viewer, else `view`; anything else is `view`. The level is fixed when the socket opens; the shell reconnects under the same peer label when the viewer's name changes.
- `room`: `kind` is always `"viewer"` and `guest` always `false`; no agent peer joins in phase 5. `by` is the sender's viewer public ID when the artifact declares `user`, else `null`. A topic declared `"interact"` in `capabilities.room.topics` admits `interact` and above; every other topic admits `admin` and above; presence admits everyone.
- **`sample` is the owner's browser's only** (owner ruling, 2026-10-02). The call and tool-result routes need the bearer token and a viewer cookie; the shell offers `sample` only when it holds the token (`isAvailable(…, owner)`), so a LAN viewer's `use("sample")` resolves `null`; every call waits on the viewer's consent (Task 8 and open question Q1). Error codes are exactly `sample.d.ts`'s `SampleErrorCode` values; the shell maps every daemon refusal into them (Task 8).
- **Time to usable.** No budget in `web/perf/bundle-budget.json` or `web/perf/budget.json` is raised (owner ruling). `room` and `sample` are new lazy bridge parts with budgets of their own (`partRoom`, `partSample`, added by `--record`, which may add a missing budget but never raise one); their shell handlers are dynamic imports, outside the artifact entry's static closure. The eager bridge grows only by the names it dispatches (Task 5 measures it against its 232-byte headroom).
- **UI.** The shell's new UI is isolated in new files (`ui/SampleCount.svelte`, the consent strings in `caps/grants.ts`) and touches existing components in one line each, so it merges after Echo with few conflicts. UI work is verified in a real browser (Playwright against a real daemon in both frame modes, plus the route loaded by hand in light mode, dark mode and at phone width) before it is called done. Open UI questions are in `.superpowers/sdd/2026-09-28-phase-5-room-and-sample/open-questions.md`; this plan builds each recommended answer and names the question beside the code it affects.

### Shared contract (part of every task's requirements)

**Room socket.** `GET /api/artifacts/<aid>/room?peer=<label>[&token=<bearer>]` upgrades to a WebSocket. `<label>` is 16 characters of `[0-9a-z]`, picked by the shell once per open document and reused on reconnect. Refused before the upgrade: 403 `forbidden_origin` (foreign `Origin`), 403 `forbidden_host` (the `/api` host rule), 400 `invalid_argument` (bad label or artifact ID). After the upgrade, a socket for a missing or undeclared artifact is closed at once. Frames are JSON text with a `t` field.

Client → daemon:

```json
{"t": "presence", "room": null, "state": {"cursor": [0.4, 0.3]}}
{"t": "emit", "id": 7, "room": "table-1", "topic": "reaction", "data": {"kind": "wave"}}
{"t": "join", "id": 8, "room": "table-1"}
{"t": "leave", "room": "table-1"}
```

`room: null` is the lobby. `state` is the whole merged presence object (the bridge merges patches). `data` is omitted when the page passed none.

Daemon → client:

```json
{"t": "welcome", "peer": "k3v6q2rt7wacd4fn"}
{"t": "peers", "room": null, "peers": [WirePeer, ...]}
{"t": "peer", "room": null, "peer": WirePeer}
{"t": "left", "room": null, "peer": "k3v6q2rt7wacd4fn"}
{"t": "msg", "room": null, "msg": {"peer": "…", "by": null, "isMe": false, "sameTab": false, "kind": "viewer", "guest": false, "topic": "reaction", "data": {"kind": "wave"}}}
{"t": "ack", "id": 7}
{"t": "ack", "id": 7, "dropped": true}
{"t": "nack", "id": 7, "code": "not_permitted", "message": "…"}
```

`WirePeer` is `{peer, by, isMe, sameTab, kind, guest, presence}`. `peers` replaces the room's list (sent on connect, after a join, and after the socket fell behind) and lists at most 256 peers, the receiver always among them; `peer` is an upsert. Close codes: 4403 with reason `not_granted` (missing or undeclared) or `revoked` (the artifact was deleted, or a new version stopped declaring `room`, while open), 4409 `replaced` (a newer socket took this label), 1001 on shutdown. Budget: emits and presence share 40 a second, burst 80; an emit past it answers `ack` with `dropped: true`; presence past it waits and is sent latest-wins. Bounds: topic `^[a-z][a-z0-9_.-]{0,47}$`, room name `^[a-z0-9][a-z0-9_.-]{0,47}$`, `data` and merged presence at most 4096 bytes of JSON and 8 levels deep, presence keys `^[A-Za-z_][A-Za-z0-9_-]{0,63}$` and never `prototype` or a name `Object.prototype` carries, at most 16 named rooms per socket, at most 16 declared topics.

**Room relay (bridge ↔ shell, ns `room`).** Calls: `connect()`, `presence(room: string | null, state)`, `emit(room, topic[, data]) → {dropped: boolean}`, `join(name)`, `leave(name)`. Pushes (`clax:event`, ns `room`): `connection {connected}`, `welcome {peer}`, `peers {room, peers}`, `peer {room, peer}`, `left {room, peer}`, `msg {room, msg}`, `error {room: string | null, code, message}` (`room: null` is terminal for the page; a name ends that room).

**Sample routes.**

| Route | Caller | Body | Answer |
|---|---|---|---|
| `GET /api/artifacts/<aid>/sample` | SameOrigin; the token decides | — | `{available, provider, limits, calls_today, daily_call_cap}`; without the token `available: false`, `provider: null` |
| `POST /api/artifacts/<aid>/sample` | SameOrigin, token, viewer cookie | `{input, verb, model_tier, tools, images, cache}` | `text/event-stream` |
| `POST /api/artifacts/<aid>/sample/<call_id>/tool_result` | SameOrigin, token, viewer cookie | `{id, content, is_error}` | 204 |
| `GET /api/sample` | token | — | `{available, provider, reason, detail, key_env, daily_call_cap}` (`clax doctor`) |

`input` is a string or `[{role, content}]`; `verb` is `text` or `json`; `model_tier` is `quick`, `default` or `complex`; `tools` is `[{name, description, input_schema?}]`; `images` is `[{media_type, data}]` (base64); `cache` is `true`, `false`, or `{gc_time_ms?, refresh?}`. `limits` is `sample.d.ts`'s `SampleLimits`: `{maxPromptBytes: 65536, tools: {maxCount: 16}}` plus `images: {maxCount: 5, maxInputBytes: 20000000, mediaTypes: ["image/jpeg", "image/png", "image/webp", "image/gif"]}` only when the provider takes images.

Refusals before the stream (JSON error, no stream): 401 `unauthorized` (no token), 403 `forbidden` (no viewer cookie), 403 `forbidden_origin`, 404 `not_found`, 403 `not_declared`, 403 `sampling_disabled` (no provider), 400 `invalid_request`, 400 `prompt_too_large`, 400 `images_unavailable`, 400 `image_rejected`, 429 `rate_limited` (the browser's queue is full, or the artifact reached `daily_call_cap`).

SSE frames, in order: one `start` `{call_id, cached, calls_today, daily_call_cap}`; then any number of `text` `{delta}` and `tool_call` `{id, name, input}`; then exactly one `done` `{text, truncated, model_tier_applied, value?}` (`value` on `verb: "json"`) or `error` `{code, message}`. Text of consecutive rounds is joined by a `text` frame whose `delta` is `"\n\n"`.

**Sample relay (bridge ↔ shell, ns `sample`).** Calls: `run(call, request)` (resolves `null` when the stream ends; rejects `{code, message}` when the daemon refused before streaming or consent was refused), `cancel(call)`, `toolResult(call, id, content, isError)`, `limits()`. Pushes: `clax:event {ns: "sample", topic: "frame", data: {call, event, data}}` where `event` and `data` are one SSE frame.

**`config.toml`.** `[serve] port` stays as it is. `[sample]` is new; every key is optional:

```toml
[serve]
port = 7480

[sample]
provider = "anthropic"          # or "stub" (tests and demos only)
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com"
max_tokens = 16000
daily_call_cap = 200            # optional; per artifact, per local day
stub_images = false             # stub only
stub_delay_ms = 40              # stub only

[sample.models]
quick = "claude-haiku-4-5-20251001"
default = "claude-sonnet-5-5"
complex = "claude-opus-5-5"
```

A `[sample]` table that is not a table, has an unknown key, names another provider, sets `max_tokens = 0`, or leaves a model ID empty is invalid: the daemon starts with sample off and logs one warning, and `clax doctor` prints a `warn` line (owner ruling). A `config.toml` that does not parse at all still stops `clax serve`, as it does today (`Cli::port_for`).

### Ruling: sandbox gate inheritance (scan B5)

The phase 4 final review parked one item for this phase: in sandbox mode the shell posts to the frame with target `*`, so live room data could reach a document the frame navigated to. Since then, `clax:bye` closes the gate on `pagehide`, `FrameGate.load()` closes it for a document that loaded without greeting, and a hello must name the shown artifact and version. **Ruling: those three close the parked item; no per-document welcome nonce is added.** The window that remains runs from the next document's commit to the shell's handling of the outgoing page's `bye`, a few milliseconds in which a push already in flight can reach the new document. Every push in it carries data the outgoing page was entitled to, and that page chose the navigation (the shell closes the gate itself before any navigation it starts), so it could have carried the same data in the URL. A nonce would not narrow this window: a `*` post reaches whatever document holds the frame. What the item still requires is done in Task 6: the capability host learns the document is gone (`CapabilityHost.leave()`, called wherever the gate closes), so the room socket closes and the peer leaves every other page at once, and `sample` aborts its calls; and a sandbox e2e test pins it. Open question Q9 asks the owner to confirm this ruling.

## Review Focus

1. A viewer reloads a tab, or the shell reconnects after a network blip or a rename, while the daemon still holds the old socket for that label: the new socket replaces the old one (closed `replaced`, never refused), and every other page sees that peer exactly once afterwards. Pinned in Task 1 (`a_second_socket_with_the_same_label_replaces_the_first`) and Task 6 (`a rename reconnects under the same label`).
2. The frame's document goes away (`clax:bye`, a load without a hello, a shell navigation) in sandbox mode: the room socket closes at once, no room event reaches the next document, and the other tabs see the peer leave. Pinned in Task 6 (`host.test.ts` "leave", and the sandbox e2e `a document that leaves takes its peer with it`).
3. The viewer closes the tab or navigates away in the middle of a streaming answer or while a tool round is waiting: the daemon drops the provider request at once (the key stops paying) and forgets the pending call. Pinned in Task 3 (`dropping_the_stream_cancels_the_call`).
4. The key is spent only from the owner's browser: a cookie without the token, or the token without a cookie, never reaches the provider; a LAN shell's `use("sample")` resolves `null` without asking anything. Pinned in Task 3 (`only_the_owners_browser_may_spend_the_key`) and Task 8 (the LAN e2e).
5. The provider answers an error that echoes the request's credentials, or a panic path formats the provider: the key never reaches a page, an SSE frame, a response or a log. Pinned in Task 2 (`provider_errors_never_carry_the_key`, `the_provider_debug_string_hides_the_key`) and Task 3 (`the_daemon_reports_its_sampler_to_the_token_only_and_never_the_key`).
6. A page whose `room` or `sample` part cannot load: `use()` resolves `null`, the shell shows its notice once, and a page that never uses either downloads neither part nor either shell chunk. Pinned in Task 5 (`bridge-degraded.test.ts`, `bridge-parts.spec.ts`) and Task 6 (the bundle and perf gates).

## Re-check before Task 2 (the Messages API)

Checked on 2026-10-02 against the `claude-api` skill (model table cached 2026-09-25):

- `claude-opus-5-5` and `claude-sonnet-5-5` are current IDs. Omitting `thinking` runs adaptive thinking on both; `{type: "disabled"}` is a 400 on both, so the request omits it. Claude Opus 5.5's default effort is `medium`; the request sets no effort.
- `tool_choice: {type: "none"}` is unaffected on both (only forced `any`/`tool` is a 400), so the last round may send it.
- **Changed since the earlier draft:** the skill lists Haiku 4.5 as `claude-haiku-4-5` and says model IDs are complete as listed, with no date suffix. The earlier controller ruling kept `claude-haiku-4-5-20251001`. Before Task 2 the controller decides between them (open question Q10; `config.toml` overrides either way); this plan writes the dated ID until then.
- The skill recommends the server-side `fallbacks` parameter and `eager_input_streaming` for Opus 5.5 and Sonnet 5.5 code. The controller settled that the request sends neither (a refusal maps to `refused`); this plan keeps that ruling.

Before starting Task 2, also check against the live reference (the `claude-api` skill's `shared/` files and the docs they link), and correct Task 2's code and mock bodies where they differ:

1. `anthropic-version: 2023-06-01` is still the header value; `x-api-key` is the header.
2. Event names and delta types: `message_start`, `content_block_start`, `content_block_delta` with `text_delta`, `input_json_delta` (`partial_json`), `thinking_delta`, `signature_delta`; `content_block_stop`; `message_delta` (`delta.stop_reason`); `message_stop`; `ping`; `error` (`{"type":"error","error":{"type","message"}}`).
3. `stop_reason` values: `end_turn`, `stop_sequence`, `max_tokens`, `tool_use`, `refusal`, `pause_turn`.
4. Echoing the assistant's content blocks verbatim in the next tool round (thinking blocks with their `signature` and empty `thinking` text, which is the default display on these models) is accepted, and the request history is append-only (preserved thinking rejects edited history on accounts created after 2026-08-31).
5. Error types and statuses: `invalid_request_error` 400 (and the wording of "prompt is too long"), `authentication_error` 401, `billing_error` 402, `permission_error` 403, `not_found_error` 404, `request_too_large` 413, `rate_limit_error` 429, `api_error` 500, `overloaded_error` 529.
6. Image blocks: `{"type":"image","source":{"type":"base64","media_type","data"}}`, the accepted media types, and the per-image and per-request limits (the bridge re-encodes to about 1.2 megapixels and caps five images of 5 MiB each).
7. `max_tokens` default 16000 per round.

---

## File structure

```
Cargo.toml                                              axum "ws"; tokio-tungstenite (workspace entry, dev use only)
crates/clax-core/src/room.rs                            topic/room/peer grammar, JSON bounds, reserved keys, declared Topics (Task 1)
crates/clax-core/src/config.rs                          HomeConfig::sample, SampleConfig, SampleModels (Task 2)
crates/clax-core/src/{lib,capabilities}.rs              module wiring, room.topics validation
crates/clax-server/src/room.rs                          the in-memory hub (Task 1)
crates/clax-server/src/routes/room.rs                   the room WebSocket (Task 1)
crates/clax-server/src/sample/{mod,provider,sse,anthropic,stub}.rs   Sampler, OffReason, providers (Task 2)
crates/clax-server/src/sample/{request,flight,json_reply}.rs         the call (Task 3)
crates/clax-server/src/sample/{cache,quota}.rs                       caching and quotas (Task 4)
crates/clax-server/src/routes/sample.rs                 status, sample, tool_result, daemon (Tasks 3–4)
crates/clax-server/src/{lib,state,daemon,testing}.rs, routes/mod.rs
crates/clax-server/tests/{api_room,sample_anthropic,api_sample,daemon}.rs
crates/clax-cli/src/commands/{serve,doctor}.rs
web/bridge/src/{capabilities,use,bridge,protocol,parts-url,parts-static}.ts, parts/types.ts     part dispatch (Task 5)
web/bridge/src/parts/{room,sample}.ts, caps/{room,sample}.ts                                       the new parts (Tasks 5, 7)
web/bridge/test/{room,sample,capabilities,use,bridge-degraded,part-loader}.test.ts
web/scripts/{build-parts,bundle-size}.mjs, web/vite.bridge.config.ts, web/perf/bundle-budget.json
web/shell/src/caps/{lazy,room,sample,availability,grants,host,registry}.ts, web/shell/src/{sse,api,failure}.ts
web/shell/src/view/artifact-controller.ts, web/shell/src/ui/{SampleCount.svelte,TopbarIsland.svelte}
web/shell/src/caps/{lazy,room,sample,grants,host}.test.ts, web/shell/src/sse.test.ts, web/shell/src/view/artifact-controller.test.ts
web/e2e/{fixtures,room.spec,sample.spec,contract.spec,bridge-parts.spec}.ts, web/e2e/pages/{room,sample}.html
docs/superpowers/specs/2026-09-28-clax-design.md, docs/contract.md, plugins/{claude-code,clax,pi}/skills/clax/SKILL.md
scripts/{test-plugins,smoke-capabilities,demo-room-sample,test-justfile}.sh, justfile, README.md
```

---

### Task 1: The room server: grammar, declarations, the hub, and the WebSocket

Carried over from the earlier draft of this plan, renamed, with four corrections: presence keys refuse `prototype` and the `Object.prototype` names (scan F13); a `peers` snapshot holds at most 256 peers (N6); a socket closes `revoked` when a new version stops declaring `room` (N5); and `Subscriber` is used as built (F3).

**Files:**
- Modify: `Cargo.toml` (axum `ws`; workspace entry `tokio-tungstenite`)
- Create: `crates/clax-core/src/room.rs`
- Modify: `crates/clax-core/src/lib.rs` (`pub mod room;`), `crates/clax-core/src/capabilities.rs` (`validate` checks `room.topics`)
- Create: `crates/clax-server/src/room.rs`, `crates/clax-server/src/routes/room.rs`
- Modify: `crates/clax-server/src/lib.rs` (`pub mod room;`), `crates/clax-server/src/routes/mod.rs` (`pub mod room;` and the route), `crates/clax-server/src/state.rs` (`rooms`), `crates/clax-server/src/daemon.rs` and `crates/clax-server/src/testing.rs` (construct `rooms`), `crates/clax-server/Cargo.toml` (dev-dependency `tokio-tungstenite`)
- Test: unit tests in `crates/clax-core/src/room.rs`, `crates/clax-core/src/capabilities.rs`, `crates/clax-server/src/room.rs`, `crates/clax-server/src/routes/room.rs`; `crates/clax-server/tests/api_room.rs` (new)

**Interfaces:**
- Consumes (phase 4): `clax_core::db::{Level, Caller}`, `clax_core::capabilities::validate`, `clax_server::db_caller::Subscriber` with `Subscriber::resolve(&self, st: &Store) -> clax_core::Result<Caller>`, as built (`crates/clax-server/src/db_caller.rs`: it reads the `?token=<bearer>` query parameter and the viewer cookie, and applies the level rule of Global Constraints; there is no `ConnectInfo`), `TestServer::viewer(&self, name: Option<&str>) -> TestViewer { cookie, public_id }`; phase 1–3 `SameOrigin`, `parse_id`, `Event::{ArtifactDeleted, Version}`, `Store::get_artifact`, `AppState::{events, shutdown, store_call}`.
- Produces:
  - `clax_core::room::{MAX_TOPICS, MAX_JSON_BYTES, MAX_DEPTH, MAX_JOINED, PEER_LABEL_LEN, RESERVED_KEYS, topic_ok, room_name_ok, peer_label_ok, identifier_ok, depth, check_json, check_presence, Topics}`; `Topics::from_capabilities(caps: &Value) -> Result<Topics>`, `Topics::send_level(&self, topic: &str) -> Level`.
  - `clax_server::room::{ROOM_CHANNEL, MAX_SNAPSHOT, Rooms, Membership, Claim, Frame, Who, Sender}`; `Rooms::with_capacity(n: usize) -> Rooms`, `Rooms::enter(self: &Arc<Self>, aid: &str, name: Option<&str>, who: Who) -> (Membership, broadcast::Receiver<Arc<Frame>>)`, `Rooms::claim(self: &Arc<Self>, aid: &str, peer: &str) -> Claim`, `Rooms::room_count(&self) -> usize`; `Membership::{snapshot, set_presence, emit}`; `Sender::render(who: &Who, me: &Who) -> Sender`.
  - `AppState.rooms: Arc<clax_server::room::Rooms>`.
  - `clax_server::routes::room::{room, Budget, SEND_RATE, SEND_BURST}`; the route `GET /api/artifacts/{aid}/room` and the frames of the Shared contract.

- [ ] **Step 1: Add the dependencies**

In the root `Cargo.toml`, change the axum line and add the WebSocket client:

```toml
axum = { version = "0.8", features = ["multipart", "macros", "ws"] }
tokio-tungstenite = "0.26"
```

In `crates/clax-server/Cargo.toml`, under `[dev-dependencies]`, add `tokio-tungstenite.workspace = true`. Run `cargo tree -p clax-server -i tokio-tungstenite -e normal` and set the workspace `tokio-tungstenite` to the version axum itself uses (the tests pass `Message::Text(String::into())` and read `Utf8Bytes`, which is 0.26's API; if axum resolves 0.24, use `Message::Text(s)` with a `String` and `t` as `&str` in the helpers below).

Run: `cargo build --workspace --all-targets`
Expected: builds.

- [ ] **Step 2: Write the failing core tests**

Create `crates/clax-core/src/room.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Level;
    use serde_json::json;

    #[test]
    fn topics_and_room_names_follow_the_contract_grammar() {
        for ok in ["reaction", "a", "chat.v2", "x_y-z", &"t".repeat(48)] {
            assert!(topic_ok(ok), "{ok}");
        }
        for bad in ["", "Reaction", "2fast", "a:b", "a b", "é", &"t".repeat(49)] {
            assert!(!topic_ok(bad), "{bad}");
        }
        for ok in ["game-7", "7", "table.1"] {
            assert!(room_name_ok(ok), "{ok}");
        }
        for bad in ["", "-x", "Table", "a/b", &"r".repeat(49)] {
            assert!(!room_name_ok(bad), "{bad}");
        }
        assert!(peer_label_ok("k3v6q2rt7wacd4fn"));
        for bad in ["k3v6q2rt7wacd4f", "K3V6Q2RT7WACD4FN", "k3v6q2rt7wacd4f!", "k3v6q2rt7wacd4fnn"] {
            assert!(!peer_label_ok(bad), "{bad}");
        }
    }

    #[test]
    fn json_bounds_are_4096_bytes_and_8_levels() {
        assert!(check_json("data", &json!({"a": "x".repeat(4000)})).is_ok());
        let err = check_json("data", &json!({"a": "x".repeat(4100)})).unwrap_err();
        assert!(err.to_string().contains("4096"), "{err}");
        let mut deep = json!(1);
        for _ in 0..7 {
            deep = json!([deep]);
        }
        assert_eq!(depth(&deep), 8);
        assert!(check_json("data", &deep).is_ok());
        assert!(check_json("data", &json!([deep])).is_err());
    }

    #[test]
    fn presence_is_an_object_with_identifier_keys() {
        assert!(check_presence(&json!({"cursor": [0.1, 0.2], "_x": 1, "a-b": null})).is_ok());
        for bad in [json!([1]), json!("x"), json!({"1a": 1}), json!({"a b": 1}), json!({"": 1})] {
            assert!(check_presence(&bad).is_err(), "{bad}");
        }
        for reserved in ["prototype", "__proto__", "constructor", "toString", "valueOf", "hasOwnProperty", "__defineGetter__"] {
            assert!(!identifier_ok(reserved), "{reserved}");
            assert!(check_presence(&json!({reserved: 1})).is_err(), "{reserved}");
        }
    }

    #[test]
    fn declared_topics_open_interact_and_default_to_admin() {
        let t = Topics::from_capabilities(&json!({"room": {"topics": {"reaction": "interact", "clear": "admin"}}})).unwrap();
        assert_eq!(t.send_level("reaction"), Level::Interact);
        assert_eq!(t.send_level("clear"), Level::Admin);
        assert_eq!(t.send_level("other"), Level::Admin);
        assert_eq!(Topics::from_capabilities(&json!({})).unwrap(), Topics::default());
        assert_eq!(Topics::from_capabilities(&json!({"room": {}})).unwrap(), Topics::default());
    }

    #[test]
    fn malformed_topic_declarations_are_invalid_capabilities() {
        let many: serde_json::Map<String, serde_json::Value> =
            (0..17).map(|i| (format!("t{i}"), json!("interact"))).collect();
        for bad in [
            json!({"room": {"topics": []}}),
            json!({"room": {"topics": {"Bad": "interact"}}}),
            json!({"room": {"topics": {"a:b": "interact"}}}),
            json!({"room": {"topics": {"chat": "view"}}}),
            json!({"room": {"topics": {"chat": true}}}),
            json!({"room": {"topics": many}}),
        ] {
            let e = Topics::from_capabilities(&bad).unwrap_err();
            assert!(matches!(e, crate::CoreError::Invalid { code: "invalid_capabilities", .. }), "{bad}: {e}");
        }
    }
}
```

In `crates/clax-core/src/capabilities.rs`, add to `refuses_malformed_declarations`'s list:

```rust
            json!({"room": {"topics": {"chat": "everyone"}}}),
            json!({"room": {"topics": {"Chat": "interact"}}}),
```

Add `pub mod room;` to `crates/clax-core/src/lib.rs`.

- [ ] **Step 3: Run the core tests to verify they fail**

Run: `cargo test -p clax-core room`
Expected: FAIL to compile (`topic_ok`, `Topics`, … not found).

- [ ] **Step 4: Implement `clax_core::room`**

Put above the test module in `crates/clax-core/src/room.rs`:

```rust
//! The `room` capability's grammar and bounds (contract 0.2.61 `room.d.ts`):
//! topics, room names, peer labels, the size and depth of presence and
//! message data, and the declared topic levels. Rooms themselves live in the
//! daemon's memory (`clax_server::room`); nothing here is stored.

use crate::db::Level;
use crate::{CoreError, Result};
use serde_json::Value;
use std::collections::BTreeMap;

/// Most topics `capabilities.room.topics` may open.
pub const MAX_TOPICS: usize = 16;
/// Largest message `data`, and largest merged presence object, in UTF-8 bytes of JSON text.
pub const MAX_JSON_BYTES: usize = 4096;
/// Deepest nesting of message data or presence, counting the value itself.
pub const MAX_DEPTH: usize = 8;
/// Most named rooms one socket may be in at once.
pub const MAX_JOINED: usize = 16;
/// Length of a peer label.
pub const PEER_LABEL_LEN: usize = 16;

fn grammar(s: &str, first: impl Fn(u8) -> bool) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 48
        && first(b[0])
        && b[1..]
            .iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'.' | b'-'))
}

/// `^[a-z][a-z0-9_.-]{0,47}$`: colon-free, so no page can forge a platform kind.
pub fn topic_ok(t: &str) -> bool {
    grammar(t, |c| c.is_ascii_lowercase())
}

/// `^[a-z0-9][a-z0-9_.-]{0,47}$`.
pub fn room_name_ok(n: &str) -> bool {
    grammar(n, |c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Sixteen characters of `[0-9a-z]`: the label a shell picks for one open document.
pub fn peer_label_ok(p: &str) -> bool {
    p.len() == PEER_LABEL_LEN && p.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Names a presence key may not take (`room.d.ts`): `prototype` and every
/// name `Object.prototype` carries.
pub const RESERVED_KEYS: &[&str] = &[
    "prototype",
    "__proto__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

/// `^[A-Za-z_][A-Za-z0-9_-]{0,63}$` and not in [`RESERVED_KEYS`]: a presence key.
pub fn identifier_ok(k: &str) -> bool {
    let b = k.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        && !RESERVED_KEYS.contains(&k)
}

/// Nesting depth of `v`: 1 for a scalar, one more per array or object level.
pub fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 1,
    }
}

/// Checks `v` (named `what` in the message) against [`MAX_JSON_BYTES`] and [`MAX_DEPTH`].
///
/// # Errors
/// `invalid_argument` naming the bound that was passed.
pub fn check_json(what: &str, v: &Value) -> Result<()> {
    let bytes = serde_json::to_string(v).map_or(usize::MAX, |s| s.len());
    if bytes > MAX_JSON_BYTES {
        return Err(CoreError::invalid(
            "invalid_argument",
            format!("{what} is {bytes} bytes of JSON; the limit is {MAX_JSON_BYTES}"),
        ));
    }
    if depth(v) > MAX_DEPTH {
        return Err(CoreError::invalid(
            "invalid_argument",
            format!("{what} nests deeper than {MAX_DEPTH} levels"),
        ));
    }
    Ok(())
}

/// A whole presence object: a JSON object whose keys pass [`identifier_ok`],
/// within [`check_json`]'s bounds.
///
/// # Errors
/// `invalid_argument` naming the problem.
pub fn check_presence(v: &Value) -> Result<()> {
    let obj = v
        .as_object()
        .ok_or_else(|| CoreError::invalid("invalid_argument", "presence must be a JSON object"))?;
    if let Some(k) = obj.keys().find(|k| !identifier_ok(k)) {
        return Err(CoreError::invalid(
            "invalid_argument",
            format!("presence key '{k}' is not an identifier"),
        ));
    }
    check_json("presence", v)
}

/// The declared `capabilities.room.topics`: the topics the `interact` level may
/// send on. Every topic not listed as `"interact"` needs `admin`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Topics(BTreeMap<String, Level>);

impl Topics {
    /// Reads `capabilities.room.topics`: an object of at most [`MAX_TOPICS`]
    /// topics ([`topic_ok`]), each `"interact"` or `"admin"`. A missing `room`
    /// or `topics` opens nothing.
    ///
    /// # Errors
    /// `invalid_capabilities` naming the first problem.
    pub fn from_capabilities(caps: &Value) -> Result<Topics> {
        let bad = |m: String| CoreError::invalid("invalid_capabilities", m);
        let Some(topics) = caps.get("room").and_then(|r| r.get("topics")) else {
            return Ok(Topics::default());
        };
        let obj = topics
            .as_object()
            .ok_or_else(|| bad("capabilities.room.topics must be an object of topic to level".into()))?;
        if obj.len() > MAX_TOPICS {
            return Err(bad(format!(
                "capabilities.room.topics lists {} topics; the limit is {MAX_TOPICS}",
                obj.len()
            )));
        }
        let mut out = BTreeMap::new();
        for (t, level) in obj {
            if !topic_ok(t) {
                return Err(bad(format!(
                    "capabilities.room.topics: '{t}' is not a topic (^[a-z][a-z0-9_.-]{{0,47}}$)"
                )));
            }
            let level = match level.as_str() {
                Some("interact") => Level::Interact,
                Some("admin") => Level::Admin,
                _ => return Err(bad(format!("capabilities.room.topics.{t} must be \"interact\" or \"admin\""))),
            };
            out.insert(t.clone(), level);
        }
        Ok(Topics(out))
    }

    /// The lowest level that may send on `topic`.
    pub fn send_level(&self, topic: &str) -> Level {
        self.0.get(topic).copied().unwrap_or(Level::Admin)
    }
}
```

In `crates/clax-core/src/capabilities.rs`, at the end of `validate` before `Ok(())`:

```rust
    crate::room::Topics::from_capabilities(caps)?;
```

and extend its doc comment: "`room.topics` must pass [`crate::room::Topics::from_capabilities`]."

- [ ] **Step 5: Run the core tests to verify they pass**

Run: `cargo test -p clax-core && cargo clippy -p clax-core --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 6: Write the failing hub tests**

Create `crates/clax-server/src/room.rs` with the test module only, and add `pub mod room;` to `crates/clax-server/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::broadcast::error::TryRecvError;

    fn who(peer: &str, viewer: Option<&str>) -> Who {
        Who { peer: peer.into(), viewer: viewer.map(Into::into), by: None }
    }

    fn obj(v: serde_json::Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn entering_announces_the_peer_and_snapshots_everyone() {
        let rooms = Arc::new(Rooms::default());
        let (a, mut ra) = rooms.enter("7q3k9mzx2b4t", None, who("aaaaaaaaaaaaaaaa", Some("u_a")));
        let (b, _rb) = rooms.enter("7q3k9mzx2b4t", None, who("bbbbbbbbbbbbbbbb", Some("u_b")));
        let peers: Vec<String> = b.snapshot().into_iter().map(|(w, _)| w.peer).collect();
        assert_eq!(peers, ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"]);
        assert!(matches!(&*ra.try_recv().unwrap(), Frame::Peer { who, .. } if who.peer == "aaaaaaaaaaaaaaaa"));
        assert!(matches!(&*ra.try_recv().unwrap(), Frame::Peer { who, .. } if who.peer == "bbbbbbbbbbbbbbbb"));
        b.set_presence(obj(json!({"pick": "B"})));
        assert!(matches!(&*ra.try_recv().unwrap(), Frame::Peer { presence, .. } if presence["pick"] == "B"));
        a.emit("reaction".into(), Some(json!({"k": 1})));
        assert!(matches!(&*ra.try_recv().unwrap(), Frame::Msg { topic, .. } if topic == "reaction"));
    }

    #[test]
    fn named_rooms_and_artifacts_are_separate() {
        let rooms = Arc::new(Rooms::default());
        let (_lobby, mut rl) = rooms.enter("7q3k9mzx2b4t", None, who("aaaaaaaaaaaaaaaa", None));
        let (t1, _r1) = rooms.enter("7q3k9mzx2b4t", Some("table-1"), who("bbbbbbbbbbbbbbbb", None));
        let (_other, mut ro) = rooms.enter("zzzzzzzzzzzz", None, who("cccccccccccccccc", None));
        let _ = rl.try_recv();
        let _ = ro.try_recv();
        t1.emit("reaction".into(), None);
        assert!(matches!(rl.try_recv(), Err(TryRecvError::Empty)));
        assert!(matches!(ro.try_recv(), Err(TryRecvError::Empty)));
        assert_eq!(rooms.room_count(), 3);
    }

    #[test]
    fn dropping_a_membership_leaves_and_an_empty_room_disappears() {
        let rooms = Arc::new(Rooms::default());
        let (a, mut ra) = rooms.enter("7q3k9mzx2b4t", None, who("aaaaaaaaaaaaaaaa", None));
        let (b, _rb) = rooms.enter("7q3k9mzx2b4t", None, who("bbbbbbbbbbbbbbbb", None));
        while ra.try_recv().is_ok() {}
        drop(b);
        assert!(matches!(&*ra.try_recv().unwrap(), Frame::Left { peer } if peer == "bbbbbbbbbbbbbbbb"));
        assert_eq!(a.snapshot().len(), 1);
        drop(a);
        assert_eq!(rooms.room_count(), 0);
    }

    #[test]
    fn a_lagging_receiver_is_told_and_the_snapshot_is_still_whole() {
        let rooms = Arc::new(Rooms::with_capacity(2));
        let (_a, mut ra) = rooms.enter("7q3k9mzx2b4t", None, who("aaaaaaaaaaaaaaaa", None));
        let (b, _rb) = rooms.enter("7q3k9mzx2b4t", None, who("bbbbbbbbbbbbbbbb", None));
        for i in 0..5 {
            b.set_presence(obj(json!({"n": i})));
        }
        assert!(matches!(ra.try_recv(), Err(TryRecvError::Lagged(_))));
        let snap = b.snapshot();
        assert_eq!(snap.iter().find(|(w, _)| w.peer == "bbbbbbbbbbbbbbbb").unwrap().1["n"], 4);
    }

    #[test]
    fn a_snapshot_holds_at_most_256_peers_and_always_the_caller() {
        let rooms = Arc::new(Rooms::default());
        let mut held = Vec::new();
        for i in 0..300 {
            held.push(rooms.enter("7q3k9mzx2b4t", None, who(&format!("p{i:015}"), None)));
        }
        let (me, _rx) = rooms.enter("7q3k9mzx2b4t", None, who("zzzzzzzzzzzzzzzz", None));
        let snap = me.snapshot();
        assert_eq!(snap.len(), MAX_SNAPSHOT);
        assert!(snap.iter().any(|(w, _)| w.peer == "zzzzzzzzzzzzzzzz"));
    }

    #[test]
    fn a_later_entry_with_the_same_label_takes_it_over_silently() {
        let rooms = Arc::new(Rooms::default());
        let (_watch, mut rw) = rooms.enter("7q3k9mzx2b4t", None, who("wwwwwwwwwwwwwwww", None));
        let (old, _ro) = rooms.enter("7q3k9mzx2b4t", None, who("aaaaaaaaaaaaaaaa", Some("u_a")));
        let (new, _rn) = rooms.enter("7q3k9mzx2b4t", None, who("aaaaaaaaaaaaaaaa", Some("u_a")));
        old.set_presence(obj(json!({"stale": true})));
        drop(old);
        while let Ok(f) = rw.try_recv() {
            assert!(!matches!(&*f, Frame::Left { .. }), "the replaced membership must leave silently");
            assert!(!matches!(&*f, Frame::Peer { presence, .. } if presence.contains_key("stale")));
        }
        assert_eq!(new.snapshot().len(), 2);
    }

    #[tokio::test]
    async fn a_newer_claim_on_a_label_notifies_the_older_one() {
        let rooms = Arc::new(Rooms::default());
        let first = rooms.claim("7q3k9mzx2b4t", "aaaaaaaaaaaaaaaa");
        let second = rooms.claim("7q3k9mzx2b4t", "aaaaaaaaaaaaaaaa");
        tokio::time::timeout(std::time::Duration::from_secs(1), first.replaced.notified())
            .await
            .expect("the first claim hears it was replaced");
        drop(first);
        let third = rooms.claim("7q3k9mzx2b4t", "bbbbbbbbbbbbbbbb");
        drop(third);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(50), second.replaced.notified()).await.is_err());
    }

    #[test]
    fn senders_render_is_me_and_same_tab_per_recipient() {
        let a1 = Who { peer: "aaaaaaaaaaaaaaaa".into(), viewer: Some("u_a".into()), by: Some("u_a".into()) };
        let a2 = Who { peer: "cccccccccccccccc".into(), viewer: Some("u_a".into()), by: Some("u_a".into()) };
        let b = Who { peer: "bbbbbbbbbbbbbbbb".into(), viewer: Some("u_b".into()), by: None };
        let anon = Who { peer: "dddddddddddddddd".into(), viewer: None, by: None };
        let s = Sender::render(&a1, &a1);
        assert!(s.is_me && s.same_tab);
        let s = Sender::render(&a1, &a2);
        assert!(s.is_me && !s.same_tab);
        let s = Sender::render(&a1, &b);
        assert!(!s.is_me && !s.same_tab);
        assert_eq!(s.by.as_deref(), Some("u_a"));
        assert!(!Sender::render(&anon, &Who { peer: "eeeeeeeeeeeeeeee".into(), viewer: None, by: None }).is_me);
        let v = serde_json::to_value(Sender::render(&b, &a1)).unwrap();
        assert_eq!(v, json!({"peer": "bbbbbbbbbbbbbbbb", "by": null, "isMe": false, "sameTab": false, "kind": "viewer", "guest": false}));
    }
}
```

- [ ] **Step 7: Run the hub tests to verify they fail**

Run: `cargo test -p clax-server --lib room`
Expected: FAIL to compile (`Rooms`, `Who`, … not found).

- [ ] **Step 8: Implement the hub**

Put above the test module in `crates/clax-server/src/room.rs`:

```rust
//! In-memory rooms for the `room` capability (spec §9; contract `room.d.ts`).
//! A room is keyed by artifact and name (`None` is the lobby); it holds each
//! peer's presence and fans frames out on a bounded broadcast channel, so a
//! subscriber that falls behind loses the oldest frames (it is then sent a
//! fresh snapshot, see `routes::room`). Nothing is persisted: a room
//! disappears with its last peer.

use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, broadcast};

/// Frames a room buffers for each subscriber before the oldest are dropped.
pub const ROOM_CHANNEL: usize = 256;

/// Most peers one `peers` snapshot lists (`room.d.ts`: "complete up to about
/// 256"); the snapshot's own peer is always among them.
pub const MAX_SNAPSHOT: usize = 256;

/// One peer as the daemon knows it. `viewer` (the viewer's public ID) only
/// decides `isMe` and is never sent; `by` is what other pages see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Who {
    pub peer: String,
    pub viewer: Option<String>,
    /// The viewer's public ID when the artifact declares `user`, else `None`.
    pub by: Option<String>,
}

/// A frame fanned out to one room's subscribers.
#[derive(Clone, Debug)]
pub enum Frame {
    /// A peer entered, or its presence changed (an upsert of the whole object).
    Peer { who: Who, presence: Map<String, Value> },
    Left { peer: String },
    Msg { who: Who, topic: String, data: Option<Value> },
}

/// The sender fields of every delivered peer and message (`room.d.ts`
/// `Sender`), as one recipient sees them.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sender {
    pub peer: String,
    pub by: Option<String>,
    pub is_me: bool,
    pub same_tab: bool,
    pub kind: &'static str,
    pub guest: bool,
}

impl Sender {
    /// `who` as `me` sees it: `sameTab` when it is the same peer, `isMe` when
    /// it is the same peer or the same known viewer. `kind` is always
    /// `"viewer"` and `guest` always `false`.
    pub fn render(who: &Who, me: &Who) -> Sender {
        let same_tab = who.peer == me.peer;
        let is_me = same_tab || (who.viewer.is_some() && who.viewer == me.viewer);
        Sender {
            peer: who.peer.clone(),
            by: who.by.clone(),
            is_me,
            same_tab,
            kind: "viewer",
            guest: false,
        }
    }
}

struct Room {
    tx: broadcast::Sender<Arc<Frame>>,
    /// By peer label: the membership serial that holds the label, the peer, its presence.
    peers: Mutex<BTreeMap<String, (u64, Who, Map<String, Value>)>>,
}

type Key = (String, Option<String>);

/// Every live room of the daemon, and which socket holds each peer label.
pub struct Rooms {
    capacity: usize,
    rooms: Mutex<HashMap<Key, Arc<Room>>>,
    claims: Mutex<HashMap<(String, String), (u64, Arc<Notify>)>>,
    next_claim: AtomicU64,
    next_serial: AtomicU64,
}

impl Default for Rooms {
    fn default() -> Self {
        Rooms::with_capacity(ROOM_CHANNEL)
    }
}

impl Rooms {
    /// Rooms whose channels buffer `capacity` frames per subscriber.
    pub fn with_capacity(capacity: usize) -> Rooms {
        Rooms {
            capacity,
            rooms: Mutex::default(),
            claims: Mutex::default(),
            next_claim: AtomicU64::new(0),
            next_serial: AtomicU64::new(0),
        }
    }

    /// Puts `who` in room `name` of artifact `aid` (opening it if empty) with
    /// an empty presence, announces it to everyone there, and returns the
    /// membership and a receiver subscribed before the announcement. Dropping
    /// the membership leaves. A later `enter` with the same peer label takes
    /// the label over: the earlier membership then leaves silently, so the
    /// room shows that peer once.
    pub fn enter(self: &Arc<Self>, aid: &str, name: Option<&str>, who: Who) -> (Membership, broadcast::Receiver<Arc<Frame>>) {
        let key: Key = (aid.to_string(), name.map(str::to_string));
        let mut rooms = self.rooms.lock().expect("rooms lock");
        let room = rooms
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(Room {
                    tx: broadcast::channel(self.capacity).0,
                    peers: Mutex::default(),
                })
            })
            .clone();
        let rx = room.tx.subscribe();
        let serial = self.next_serial.fetch_add(1, Ordering::Relaxed);
        room.peers
            .lock()
            .expect("peers lock")
            .insert(who.peer.clone(), (serial, who.clone(), Map::new()));
        let _ = room.tx.send(Arc::new(Frame::Peer { who: who.clone(), presence: Map::new() }));
        drop(rooms);
        (Membership { rooms: self.clone(), key, room, who, serial }, rx)
    }

    /// Takes peer label `peer` of artifact `aid` for one socket. A later claim
    /// of the same label notifies this one's `replaced`.
    pub fn claim(self: &Arc<Self>, aid: &str, peer: &str) -> Claim {
        let key = (aid.to_string(), peer.to_string());
        let generation = self.next_claim.fetch_add(1, Ordering::Relaxed);
        let replaced = Arc::new(Notify::new());
        let old = self
            .claims
            .lock()
            .expect("claims lock")
            .insert(key.clone(), (generation, replaced.clone()));
        if let Some((_, n)) = old {
            n.notify_one();
        }
        Claim { rooms: self.clone(), key, generation, replaced }
    }

    /// How many rooms are open (tests).
    pub fn room_count(&self) -> usize {
        self.rooms.lock().expect("rooms lock").len()
    }
}

/// One socket's hold on a peer label; see [`Rooms::claim`].
pub struct Claim {
    rooms: Arc<Rooms>,
    key: (String, String),
    generation: u64,
    pub replaced: Arc<Notify>,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut claims = self.rooms.claims.lock().expect("claims lock");
        if claims.get(&self.key).is_some_and(|(g, _)| *g == self.generation) {
            claims.remove(&self.key);
        }
    }
}

/// A peer's place in one room; dropping it leaves the room.
pub struct Membership {
    rooms: Arc<Rooms>,
    key: Key,
    room: Arc<Room>,
    who: Who,
    serial: u64,
}

impl Membership {
    /// Everyone in the room now, ordered by peer label, at most
    /// [`MAX_SNAPSHOT`] of them, this peer always included.
    pub fn snapshot(&self) -> Vec<(Who, Map<String, Value>)> {
        let peers = self.room.peers.lock().expect("peers lock");
        let mine = peers
            .get(&self.who.peer)
            .filter(|(s, ..)| *s == self.serial)
            .map(|(_, w, p)| (w.clone(), p.clone()));
        let others = MAX_SNAPSHOT - usize::from(mine.is_some());
        let mut out: Vec<(Who, Map<String, Value>)> = peers
            .values()
            .filter(|(_, w, _)| w.peer != self.who.peer)
            .take(others)
            .map(|(_, w, p)| (w.clone(), p.clone()))
            .collect();
        out.extend(mine);
        out.sort_by(|a, b| a.0.peer.cmp(&b.0.peer));
        out
    }

    /// Replaces this peer's presence object and announces it.
    pub fn set_presence(&self, presence: Map<String, Value>) {
        match self.room.peers.lock().expect("peers lock").get_mut(&self.who.peer) {
            Some(entry) if entry.0 == self.serial => entry.2 = presence.clone(),
            _ => return,
        }
        let _ = self.room.tx.send(Arc::new(Frame::Peer { who: self.who.clone(), presence }));
    }

    /// Sends a moment to everyone in the room, this peer included.
    pub fn emit(&self, topic: String, data: Option<Value>) {
        let _ = self.room.tx.send(Arc::new(Frame::Msg { who: self.who.clone(), topic, data }));
    }
}

impl Drop for Membership {
    fn drop(&mut self) {
        let mut rooms = self.rooms.rooms.lock().expect("rooms lock");
        let (held, empty) = {
            let mut peers = self.room.peers.lock().expect("peers lock");
            // A later membership with this label took it over: leave silently.
            let held = peers.get(&self.who.peer).is_some_and(|(s, ..)| *s == self.serial);
            if held {
                peers.remove(&self.who.peer);
            }
            (held, peers.is_empty())
        };
        if held {
            let _ = self.room.tx.send(Arc::new(Frame::Left { peer: self.who.peer.clone() }));
        }
        if empty && rooms.get(&self.key).is_some_and(|r| Arc::ptr_eq(r, &self.room)) {
            rooms.remove(&self.key);
        }
    }
}
```

- [ ] **Step 9: Run the hub tests to verify they pass**

Run: `cargo test -p clax-server --lib room`
Expected: PASS.

- [ ] **Step 10: Write the failing socket tests**

Create `crates/clax-server/tests/api_room.rs`:

```rust
mod common;
use clax_server::testing::TestViewer;
use common::TestServer;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client {
    ws: Ws,
}

impl Client {
    async fn send(&mut self, v: Value) {
        self.ws.send(Message::Text(v.to_string().into())).await.unwrap();
    }

    /// The next frame within `ms`, or `None`; a close is `{"t": "_close", code, reason}`.
    async fn next_within(&mut self, ms: u64) -> Option<Value> {
        loop {
            let m = tokio::time::timeout(Duration::from_millis(ms), self.ws.next()).await.ok()??;
            match m.expect("readable") {
                Message::Text(t) => return Some(serde_json::from_str(t.as_str()).unwrap()),
                Message::Close(c) => {
                    let c = c.expect("a close frame");
                    return Some(json!({"t": "_close", "code": u16::from(c.code), "reason": c.reason.as_str()}));
                }
                _ => continue,
            }
        }
    }

    async fn next(&mut self) -> Value {
        self.next_within(5000).await.expect("a frame within 5 s")
    }

    /// Skips frames until one satisfies `pred`.
    async fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        loop {
            let f = self.next().await;
            if pred(&f) {
                return f;
            }
        }
    }
}

const A: &str = "aaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbb";
const C: &str = "cccccccccccccccc";

/// The socket URL; `token` is appended as the owner shell appends it.
fn url(ts: &TestServer, aid: &str, peer: &str, token: Option<&str>) -> String {
    let base = format!("{}/api/artifacts/{aid}/room?peer={peer}", ts.base.replacen("http://", "ws://", 1));
    match token {
        Some(t) => format!("{base}&token={t}"),
        None => base,
    }
}

async fn try_connect(ts: &TestServer, aid: &str, peer: &str, token: Option<&str>, headers: &[(&str, String)]) -> Result<Client, u16> {
    let mut req = url(ts, aid, peer, token).into_client_request().unwrap();
    for (k, v) in headers {
        req.headers_mut().insert(*k, v.parse().unwrap());
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => Ok(Client { ws }),
        Err(tokio_tungstenite::tungstenite::Error::Http(res)) => Err(res.status().as_u16()),
        Err(e) => panic!("connect: {e}"),
    }
}

async fn connect(ts: &TestServer, aid: &str, peer: &str, token: Option<&str>, headers: &[(&str, String)]) -> Client {
    let mut c = try_connect(ts, aid, peer, token, headers).await.expect("upgrade");
    assert_eq!(c.next().await, json!({"t": "welcome", "peer": peer}));
    c
}

fn cookie(v: &TestViewer) -> (&'static str, String) {
    ("cookie", format!("clax_viewer={}", v.cookie))
}

// Levels: the owner shell is the token plus its cookie (`admin`); a LAN viewer
// is a cookie without the token (`interact` when named, else `view`).

async fn artifact(ts: &TestServer, caps: Value) -> String {
    let res = ts
        .post_json(
            "/api/artifacts",
            json!({"title": "Room", "capabilities": caps, "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["artifact"]["id"].as_str().unwrap().to_string()
}

fn caps() -> Value {
    json!({"room": {"topics": {"reaction": "interact"}}, "user": {}})
}

#[tokio::test]
async fn presence_and_messages_reach_every_peer_with_sender_fields() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let (ann, ben) = (ts.viewer(Some("Ann")).await, ts.viewer(Some("Ben")).await);
    let mut a = connect(&ts, &aid, A, Some(ts.token.as_str()), &[cookie(&ann)]).await;
    let first = a.next().await;
    assert_eq!(first["t"], "peers");
    assert_eq!(first["room"], Value::Null);
    assert_eq!(first["peers"], json!([{"peer": A, "by": ann.public_id, "isMe": true, "sameTab": true, "kind": "viewer", "guest": false, "presence": {}}]));
    let mut b = connect(&ts, &aid, B, None, &[cookie(&ben)]).await;
    assert_eq!(b.next().await["peers"].as_array().unwrap().len(), 2);
    let seen = a.until(|f| f["t"] == "peer" && f["peer"]["peer"] == B).await;
    assert_eq!(seen["peer"]["isMe"], false);
    assert_eq!(seen["peer"]["by"], ben.public_id.as_str());
    b.send(json!({"t": "presence", "room": null, "state": {"cursor": [1, 2]}})).await;
    let moved = a.until(|f| f["t"] == "peer" && f["peer"]["presence"] != json!({})).await;
    assert_eq!(moved["peer"]["presence"], json!({"cursor": [1, 2]}));
    b.send(json!({"t": "emit", "id": 1, "room": null, "topic": "reaction", "data": {"kind": "wave"}})).await;
    assert_eq!(b.until(|f| f["t"] == "ack").await, json!({"t": "ack", "id": 1}));
    let msg = a.until(|f| f["t"] == "msg").await;
    assert_eq!(
        msg["msg"],
        json!({"peer": B, "by": ben.public_id, "isMe": false, "sameTab": false, "kind": "viewer", "guest": false, "topic": "reaction", "data": {"kind": "wave"}})
    );
    let echo = b.until(|f| f["t"] == "msg").await;
    assert_eq!((echo["msg"]["isMe"].clone(), echo["msg"]["sameTab"].clone()), (json!(true), json!(true)));
}

#[tokio::test]
async fn by_is_null_without_a_user_declaration() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"room": {}})).await;
    let ann = ts.viewer(Some("Ann")).await;
    let mut a = connect(&ts, &aid, A, None, &[cookie(&ann)]).await;
    assert_eq!(a.next().await["peers"][0]["by"], Value::Null);
}

#[tokio::test]
async fn admin_topics_refuse_lower_levels() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let (owner, ben, anon) = (ts.viewer(Some("Owner")).await, ts.viewer(Some("Ben")).await, ts.viewer(None).await);
    let mut a = connect(&ts, &aid, A, Some(ts.token.as_str()), &[cookie(&owner)]).await;
    let mut b = connect(&ts, &aid, B, None, &[cookie(&ben)]).await;
    let mut v = connect(&ts, &aid, C, None, &[cookie(&anon)]).await;
    b.send(json!({"t": "emit", "id": 1, "room": null, "topic": "clear"})).await;
    let n = b.until(|f| f["t"] == "nack").await;
    assert_eq!((n["id"].clone(), n["code"].clone()), (json!(1), json!("not_permitted")));
    v.send(json!({"t": "emit", "id": 2, "room": null, "topic": "reaction"})).await;
    assert_eq!(v.until(|f| f["t"] == "nack").await["code"], "not_permitted");
    v.send(json!({"t": "presence", "room": null, "state": {"pick": "A"}})).await;
    a.until(|f| f["t"] == "peer" && f["peer"]["peer"] == C && f["peer"]["presence"]["pick"] == "A").await;
    a.send(json!({"t": "emit", "id": 3, "room": null, "topic": "clear"})).await;
    assert_eq!(a.until(|f| f["t"] == "ack" || f["t"] == "nack").await, json!({"t": "ack", "id": 3}));
}

#[tokio::test]
async fn the_token_alone_is_owner_and_a_wrong_token_is_no_token() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let named = ts.viewer(Some("Ben")).await;
    let mut agent = connect(&ts, &aid, A, Some(ts.token.as_str()), &[]).await;
    agent.send(json!({"t": "emit", "id": 1, "room": null, "topic": "clear"})).await;
    assert_eq!(agent.until(|f| f["t"] == "ack" || f["t"] == "nack").await, json!({"t": "ack", "id": 1}));
    let mut forged = connect(&ts, &aid, B, Some("not-the-token"), &[cookie(&named)]).await;
    forged.send(json!({"t": "emit", "id": 2, "room": null, "topic": "clear"})).await;
    assert_eq!(forged.until(|f| f["t"] == "ack" || f["t"] == "nack").await["code"], "not_permitted");
    forged.send(json!({"t": "emit", "id": 3, "room": null, "topic": "reaction"})).await;
    assert_eq!(forged.until(|f| f["t"] == "ack" || f["t"] == "nack").await, json!({"t": "ack", "id": 3}));
    let mut nobody = connect(&ts, &aid, C, Some("not-the-token"), &[]).await;
    nobody.send(json!({"t": "emit", "id": 4, "room": null, "topic": "reaction"})).await;
    assert_eq!(nobody.until(|f| f["t"] == "ack" || f["t"] == "nack").await["code"], "not_permitted");
}

#[tokio::test]
async fn named_rooms_are_isolated_and_leaving_is_seen() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let owner = ts.viewer(Some("Owner")).await;
    let mut a = connect(&ts, &aid, A, None, &[cookie(&owner)]).await;
    let mut b = connect(&ts, &aid, B, None, &[cookie(&owner)]).await;
    let mut c = connect(&ts, &aid, C, None, &[cookie(&owner)]).await;
    a.send(json!({"t": "join", "id": 1, "room": "table-1"})).await;
    let snap = a.until(|f| f["t"] == "peers" && f["room"] == "table-1").await;
    assert_eq!(snap["peers"].as_array().unwrap().len(), 1);
    assert_eq!(a.until(|f| f["t"] == "ack").await, json!({"t": "ack", "id": 1}));
    b.send(json!({"t": "join", "id": 2, "room": "table-1"})).await;
    b.until(|f| f["t"] == "ack").await;
    a.until(|f| f["t"] == "peer" && f["room"] == "table-1" && f["peer"]["peer"] == B).await;
    b.send(json!({"t": "emit", "id": 3, "room": "table-1", "topic": "reaction"})).await;
    let m = a.until(|f| f["t"] == "msg").await;
    assert_eq!(m["room"], "table-1");
    let mut lobby_only = Vec::new();
    while let Some(f) = c.next_within(300).await {
        lobby_only.push(f);
    }
    assert!(lobby_only.iter().all(|f| f["room"] != "table-1"), "{lobby_only:?}");
    a.send(json!({"t": "leave", "room": "table-1"})).await;
    b.until(|f| f["t"] == "left" && f["room"] == "table-1" && f["peer"] == A).await;
}

#[tokio::test]
async fn grammar_and_bounds_answer_invalid_argument_and_limit_reached() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let owner = ts.viewer(Some("Owner")).await;
    let mut a = connect(&ts, &aid, A, None, &[cookie(&owner)]).await;
    a.send(json!({"t": "emit", "id": 1, "room": null, "topic": "Bad:topic"})).await;
    assert_eq!(a.until(|f| f["t"] == "nack").await["code"], "invalid_argument");
    a.send(json!({"t": "emit", "id": 2, "room": null, "topic": "reaction", "data": "x".repeat(5000)})).await;
    assert_eq!(a.until(|f| f["t"] == "nack").await["code"], "invalid_argument");
    a.send(json!({"t": "join", "id": 3, "room": "Bad"})).await;
    assert_eq!(a.until(|f| f["t"] == "nack").await["code"], "invalid_argument");
    for i in 0..16 {
        a.send(json!({"t": "join", "id": 10 + i, "room": format!("r{i}")})).await;
        a.until(|f| f["t"] == "ack").await;
    }
    a.send(json!({"t": "join", "id": 99, "room": "one-more"})).await;
    assert_eq!(a.until(|f| f["t"] == "nack").await["code"], "limit_reached");
    a.send(json!({"t": "join", "id": 100, "room": "r3"})).await;
    assert_eq!(a.until(|f| f["t"] == "ack" || f["t"] == "nack").await, json!({"t": "ack", "id": 100}));
}

#[tokio::test]
async fn is_me_spans_one_viewers_tabs() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let owner = ts.viewer(Some("Owner")).await;
    let _a = connect(&ts, &aid, A, None, &[cookie(&owner)]).await;
    let mut a2 = connect(&ts, &aid, B, None, &[cookie(&owner)]).await;
    let snap = a2.next().await;
    let other = snap["peers"].as_array().unwrap().iter().find(|p| p["peer"] == A).unwrap().clone();
    assert_eq!((other["isMe"].clone(), other["sameTab"].clone()), (json!(true), json!(false)));
}

#[tokio::test]
async fn undeclared_or_missing_artifacts_close_with_not_granted() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, json!({"db": {}})).await;
    for target in [aid.as_str(), "zzzzzzzzzzzz"] {
        let mut c = try_connect(&ts, target, A, None, &[]).await.expect("upgrade");
        assert_eq!(c.next().await, json!({"t": "_close", "code": 4403, "reason": "not_granted"}));
    }
}

#[tokio::test]
async fn deleting_the_artifact_closes_its_sockets_with_revoked() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let mut a = connect(&ts, &aid, A, None, &[]).await;
    a.next().await;
    let res = ts.authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base))).send().await.unwrap();
    assert!(res.status().is_success());
    assert_eq!(a.until(|f| f["t"] == "_close").await, json!({"t": "_close", "code": 4403, "reason": "revoked"}));
}

#[tokio::test]
async fn a_version_that_drops_room_closes_its_sockets_with_revoked() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let mut a = connect(&ts, &aid, A, None, &[]).await;
    a.next().await;
    let res = ts
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            json!({"if_version": 1, "capabilities": {"db": {}}, "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}}),
        )
        .await;
    assert!(res.status().is_success(), "{}", res.status());
    assert_eq!(a.until(|f| f["t"] == "_close").await, json!({"t": "_close", "code": 4403, "reason": "revoked"}));
}

#[tokio::test]
async fn a_second_socket_with_the_same_label_replaces_the_first() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let owner = ts.viewer(Some("Owner")).await;
    let mut old = connect(&ts, &aid, A, None, &[cookie(&owner)]).await;
    let _new = connect(&ts, &aid, A, None, &[cookie(&owner)]).await;
    assert_eq!(old.until(|f| f["t"] == "_close").await, json!({"t": "_close", "code": 4409, "reason": "replaced"}));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut later = connect(&ts, &aid, B, None, &[cookie(&owner)]).await;
    let snap = later.next().await;
    let labels: Vec<&str> = snap["peers"].as_array().unwrap().iter().map(|p| p["peer"].as_str().unwrap()).collect();
    assert_eq!(labels, [A, B]);
}

#[tokio::test]
async fn closing_a_socket_is_left_for_everyone() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let mut a = connect(&ts, &aid, A, None, &[]).await;
    let b = connect(&ts, &aid, B, None, &[]).await;
    a.until(|f| f["t"] == "peer" && f["peer"]["peer"] == B).await;
    drop(b);
    assert_eq!(a.until(|f| f["t"] == "left").await, json!({"t": "left", "room": null, "peer": B}));
}

#[tokio::test]
async fn foreign_origins_and_bad_labels_are_refused_before_the_upgrade() {
    let ts = TestServer::spawn().await;
    let aid = artifact(&ts, caps()).await;
    let port = ts.addr.port();
    let origin = ("origin", format!("http://{aid}.localhost:{port}"));
    assert_eq!(try_connect(&ts, &aid, A, None, &[origin]).await.err(), Some(403));
    assert_eq!(try_connect(&ts, &aid, A, None, &[("origin", "null".to_string())]).await.err(), Some(403));
    assert_eq!(try_connect(&ts, &aid, "short", None, &[]).await.err(), Some(400));
    assert_eq!(try_connect(&ts, &aid, A, None, &[("host", format!("rebind.example:{port}"))]).await.err(), Some(403));
}
```

- [ ] **Step 11: Run the socket tests to verify they fail**

Run: `cargo test -p clax-server --test api_room`
Expected: FAIL (the route does not exist: every upgrade answers 404 or 405).

- [ ] **Step 12: Implement the route**

`crates/clax-server/src/routes/room.rs`:

```rust
//! `GET /api/artifacts/<aid>/room?peer=<label>`: the `room` capability's
//! WebSocket (spec §6, §9; contract `room.d.ts`). The shell opens one per open
//! document of the artifact in both frame modes and reuses the document's
//! peer label when it reconnects; a page never reaches it (a foreign `Origin`
//! is refused before the upgrade). The caller's level is fixed when the
//! socket opens ([`Subscriber::resolve`]: the `?token=` query parameter, never
//! logged, with or without the viewer cookie; see `docs/contract.md` "Room
//! protocol") and decides which topics it may send on: a topic declared `"interact"` in `capabilities.room.topics` admits
//! `interact` and above, every other topic `admin` and above; presence admits
//! everyone. Frames are the JSON objects of `docs/contract.md` "Room protocol".
//!
//! Close codes: 4403 `not_granted` (the artifact is missing or does not declare
//! `room`), 4403 `revoked` (the artifact was deleted while the socket was
//! open, or a new version stopped declaring `room`), 4409 `replaced` (a newer socket took this peer label), 1001 when the
//! daemon shuts down.

use crate::db_caller::Subscriber;
use crate::error::ApiError;
use crate::room::{Frame, Membership, Rooms, Sender, Who};
use crate::routes::artifacts::parse_id;
use crate::state::AppState;
use crate::viewer::SameOrigin;
use clax_core::db::{Caller, Level};
use clax_core::room::{MAX_JOINED, Topics, check_json, check_presence, peer_label_ok, room_name_ok, topic_ok};
use clax_core::{CoreError, Event};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_stream::StreamExt;
use tokio_stream::StreamMap;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

/// Sends per second that emits and presence share, and the burst (`room.d.ts` `emit`).
pub const SEND_RATE: f64 = 40.0;
pub const SEND_BURST: f64 = 80.0;

/// A token bucket of [`SEND_RATE`] per second holding at most [`SEND_BURST`].
pub struct Budget {
    tokens: f64,
    at: Instant,
}

impl Budget {
    pub fn new(now: Instant) -> Budget {
        Budget { tokens: SEND_BURST, at: now }
    }

    /// Takes one send if the budget allows it at `now`.
    pub fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * SEND_RATE).min(SEND_BURST);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The route's own query field; `token` is read by [`Subscriber`].
#[derive(Deserialize)]
pub struct RoomQuery {
    peer: String,
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum ClientMsg {
    Presence { room: Option<String>, state: Value },
    Emit { id: u64, room: Option<String>, topic: String, #[serde(default)] data: Option<Value> },
    Join { id: u64, room: String },
    Leave { room: String },
}

type Streams = StreamMap<Option<String>, BroadcastStream<Arc<Frame>>>;

/// `GET /api/artifacts/{aid}/room`.
pub async fn room(
    State(s): State<AppState>,
    Path(aid): Path<String>,
    Query(q): Query<RoomQuery>,
    _o: SameOrigin,
    subscriber: Subscriber,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let id = parse_id(&aid)?;
    if !peer_label_ok(&q.peer) {
        return Err(ApiError::bad_request("invalid_argument", "peer is 16 characters of [0-9a-z]"));
    }
    let aid = id.as_str().to_string();
    let (artifact, caller) = s
        .store_call(move |st| Ok((st.get_artifact(&id)?, subscriber.resolve(st)?)))
        .await?;
    let declared = artifact.as_ref().filter(|a| a.capabilities.get("room").is_some());
    let topics = declared.map(|a| Topics::from_capabilities(&a.capabilities).unwrap_or_default());
    let declares_user = declared.is_some_and(|a| a.capabilities.get("user").is_some());
    let Caller { level, viewer } = caller;
    let who = Who { peer: q.peer, by: if declares_user { viewer.clone() } else { None }, viewer };
    Ok(ws.on_upgrade(move |socket| session(s, aid, who, level, topics, socket)))
}

fn close(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame { code, reason: reason.into() }))
}

fn wire_peer(who: &Who, presence: &Map<String, Value>, me: &Who) -> Value {
    let mut v = serde_json::to_value(Sender::render(who, me)).expect("sender serialises");
    v["presence"] = Value::Object(presence.clone());
    v
}

fn peers_frame(room: &Option<String>, m: &Membership, me: &Who) -> Value {
    let peers: Vec<Value> = m.snapshot().iter().map(|(w, p)| wire_peer(w, p, me)).collect();
    json!({"t": "peers", "room": room, "peers": peers})
}

fn render(room: &Option<String>, f: &Frame, me: &Who) -> Value {
    match f {
        Frame::Peer { who, presence } => json!({"t": "peer", "room": room, "peer": wire_peer(who, presence, me)}),
        Frame::Left { peer } => json!({"t": "left", "room": room, "peer": peer}),
        Frame::Msg { who, topic, data } => {
            let mut msg = serde_json::to_value(Sender::render(who, me)).expect("sender serialises");
            msg["topic"] = json!(topic);
            if let Some(d) = data {
                msg["data"] = d.clone();
            }
            json!({"t": "msg", "room": room, "msg": msg})
        }
    }
}

fn message_of(e: CoreError) -> String {
    match e {
        CoreError::Invalid { message, .. } => message,
        other => other.to_string(),
    }
}

/// One socket's rooms and send budget.
struct Conn {
    rooms: Arc<Rooms>,
    aid: String,
    who: Who,
    level: Level,
    topics: Topics,
    lobby: Membership,
    named: BTreeMap<String, Membership>,
    budget: Budget,
    /// Presence that arrived past the budget, latest per room.
    pending: BTreeMap<Option<String>, Map<String, Value>>,
}

impl Conn {
    fn member(&self, room: &Option<String>) -> Option<&Membership> {
        match room {
            None => Some(&self.lobby),
            Some(n) => self.named.get(n),
        }
    }

    fn handle(&mut self, text: &str, streams: &mut Streams) -> Vec<Value> {
        let Ok(msg) = serde_json::from_str::<ClientMsg>(text) else {
            tracing::debug!("room: ignored a malformed frame");
            return Vec::new();
        };
        match msg {
            ClientMsg::Presence { room, state } => {
                if check_presence(&state).is_err() || self.member(&room).is_none() {
                    return Vec::new();
                }
                let Value::Object(map) = state else { return Vec::new() };
                if self.budget.take(Instant::now()) {
                    self.pending.remove(&room);
                    self.member(&room).expect("checked above").set_presence(map);
                } else {
                    self.pending.insert(room, map);
                }
                Vec::new()
            }
            ClientMsg::Emit { id, room, topic, data } => {
                let nack = |code: &str, message: String| json!({"t": "nack", "id": id, "code": code, "message": message});
                if !topic_ok(&topic) {
                    return vec![nack("invalid_argument", format!("'{topic}' is not a topic (^[a-z][a-z0-9_.-]{{0,47}}$)"))];
                }
                if let Some(d) = &data
                    && let Err(e) = check_json("data", d)
                {
                    return vec![nack("invalid_argument", message_of(e))];
                }
                if self.level < self.topics.send_level(&topic) {
                    return vec![nack("not_permitted", format!("this viewer may not send on '{topic}'"))];
                }
                if self.member(&room).is_none() {
                    return vec![nack("invalid_argument", "this page is not in that room".into())];
                }
                if !self.budget.take(Instant::now()) {
                    return vec![json!({"t": "ack", "id": id, "dropped": true})];
                }
                self.member(&room).expect("checked above").emit(topic, data);
                vec![json!({"t": "ack", "id": id})]
            }
            ClientMsg::Join { id, room } => {
                if !room_name_ok(&room) {
                    return vec![json!({"t": "nack", "id": id, "code": "invalid_argument",
                        "message": format!("'{room}' is not a room name (^[a-z0-9][a-z0-9_.-]{{0,47}}$)")})];
                }
                if self.named.contains_key(&room) {
                    return vec![json!({"t": "ack", "id": id})];
                }
                if self.named.len() >= MAX_JOINED {
                    return vec![json!({"t": "nack", "id": id, "code": "limit_reached",
                        "message": format!("a page may be in at most {MAX_JOINED} named rooms")})];
                }
                let (m, rx) = self.rooms.enter(&self.aid, Some(&room), self.who.clone());
                let key = Some(room.clone());
                let snapshot = peers_frame(&key, &m, &self.who);
                streams.insert(key, BroadcastStream::new(rx));
                self.named.insert(room, m);
                vec![snapshot, json!({"t": "ack", "id": id})]
            }
            ClientMsg::Leave { room } => {
                self.pending.remove(&Some(room.clone()));
                streams.remove(&Some(room.clone()));
                self.named.remove(&room);
                Vec::new()
            }
        }
    }

    /// Sends presence that waited for the budget, while the budget allows.
    fn flush(&mut self) {
        while let Some(room) = self.pending.keys().next().cloned() {
            if !self.budget.take(Instant::now()) {
                return;
            }
            let map = self.pending.remove(&room).expect("key just read");
            if let Some(m) = self.member(&room) {
                m.set_presence(map);
            }
        }
    }
}

async fn session(s: AppState, aid: String, who: Who, level: Level, topics: Option<Topics>, mut socket: WebSocket) {
    let Some(topics) = topics else {
        let _ = socket.send(close(4403, "not_granted")).await;
        return;
    };
    let claim = s.rooms.claim(&aid, &who.peer);
    let (lobby, rx) = s.rooms.enter(&aid, None, who.clone());
    let mut streams: Streams = StreamMap::new();
    streams.insert(None, BroadcastStream::new(rx));
    let mut conn = Conn {
        rooms: s.rooms.clone(),
        aid: aid.clone(),
        who: who.clone(),
        level,
        topics,
        lobby,
        named: BTreeMap::new(),
        budget: Budget::new(Instant::now()),
        pending: BTreeMap::new(),
    };
    let mut events = s.events.subscribe();
    let mut shutdown = s.shutdown.clone();
    let mut tick = tokio::time::interval(Duration::from_millis(25));
    let hello = [json!({"t": "welcome", "peer": who.peer}), peers_frame(&None, &conn.lobby, &who)];
    for v in hello {
        if socket.send(Message::Text(v.to_string().into())).await.is_err() {
            return;
        }
    }
    loop {
        let out: Vec<Value> = tokio::select! {
            () = claim.replaced.notified() => {
                drop(conn);
                let _ = socket.send(close(4409, "replaced")).await;
                return;
            }
            Ok(ev) = events.recv() => {
                let gone = match &ev {
                    Event::ArtifactDeleted { artifact_id } => *artifact_id == aid,
                    // A new version may drop `room` (a metadata edit emits no
                    // event: it takes effect at the next connection).
                    Event::Version { artifact_id, .. } if *artifact_id == aid => {
                        let id = parse_id(&aid).expect("checked at the upgrade");
                        !matches!(
                            s.store_call(move |st| st.get_artifact(&id)).await,
                            Ok(Some(a)) if a.capabilities.get("room").is_some()
                        )
                    }
                    _ => false,
                };
                if gone {
                    drop(conn);
                    let _ = socket.send(close(4403, "revoked")).await;
                    return;
                }
                continue;
            }
            Ok(()) = shutdown.changed() => {
                if *shutdown.borrow() {
                    let _ = socket.send(close(1001, "shutting down")).await;
                    return;
                }
                continue;
            }
            Some((room, item)) = streams.next() => match item {
                Ok(frame) => vec![render(&room, &frame, &who)],
                Err(BroadcastStreamRecvError::Lagged(_)) => {
                    conn.member(&room).map(|m| vec![peers_frame(&room, m, &who)]).unwrap_or_default()
                }
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(t))) => conn.handle(t.as_str(), &mut streams),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => continue,
            },
            _ = tick.tick() => {
                conn.flush();
                continue;
            }
        };
        for v in out {
            if socket.send(Message::Text(v.to_string().into())).await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_allows_a_burst_of_80_then_40_a_second() {
        let t0 = Instant::now();
        let mut b = Budget::new(t0);
        assert!((0..80).all(|_| b.take(t0)));
        assert!(!b.take(t0));
        assert!(b.take(t0 + Duration::from_millis(25)));
        assert!(!b.take(t0 + Duration::from_millis(25)));
    }
}
```

In `crates/clax-server/src/routes/mod.rs`, add `pub mod room;` and, next to `/api/events` (outside the timeout groups, because the socket is long-lived):

```rust
        .route("/api/artifacts/{aid}/room", get(room::room))
```

In `crates/clax-server/src/state.rs` add the field

```rust
    /// The live rooms of the `room` capability (memory only).
    pub rooms: std::sync::Arc<crate::room::Rooms>,
```

and set `rooms: Arc::new(crate::room::Rooms::default())` where `AppState` is built in `daemon.rs` and `testing.rs`.

- [ ] **Step 13: Run the socket tests to verify they pass**

Run: `cargo test -p clax-server --test api_room && cargo test -p clax-server --lib room`
Expected: PASS. If `foreign_origins_and_bad_labels_are_refused_before_the_upgrade` sees 101 instead of 403 for the rebinding `Host`, the `/api` host middleware is not wrapping the route: it must, since the route is under `/api`.

- [ ] **Step 14: Run everything and stage**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: PASS.

Stage the change (`git add Cargo.toml Cargo.lock crates/clax-core crates/clax-server`); do not commit. The controller commits it with the message:

```text
Serve in-memory rooms over a per-document WebSocket with presence, topics gated by level, and named rooms
```


---

### Task 2: The `[sample]` table, the provider trait, the Anthropic provider, and the stub

Work through "Re-check before Task 2" first and fold any correction into this task's code and mock bodies. Steps 1–5 are new: they extend the `HomeConfig` reader that already exists instead of adding a second one (scan B1). Steps 6–14 are carried over from the earlier draft of this plan, renamed, with the sampler built from the home's config (`Sampler::from_home`) and an `OffReason` that `GET /api/sample` and `clax doctor` report (Task 3).

**Files:**
- Modify: `crates/clax-server/Cargo.toml` (`reqwest` becomes a normal dependency with `stream` and `rustls-tls`; `base64.workspace = true`; `test-support = ["dep:tempfile"]`)
- Modify: `crates/clax-core/src/config.rs` (`SampleConfig`, `SampleModels`, `HomeConfig::sample`; the module doc names `[sample]`)
- Create: `crates/clax-server/src/sample/mod.rs`, `provider.rs`, `sse.rs`, `anthropic.rs`, `stub.rs`
- Modify: `crates/clax-server/src/lib.rs` (`pub mod sample;`), `state.rs` (`sample`), `daemon.rs` (`ServeConfig.sample`), `testing.rs` (`Sampler::disabled()`), `crates/clax-server/tests/daemon.rs` (each `ServeConfig`), `crates/clax-cli/src/commands/serve.rs` (`Sampler::from_home`)
- Test: unit tests in `config.rs`, `sse.rs`, `anthropic.rs`, `stub.rs`, `sample/mod.rs`; `crates/clax-server/tests/sample_anthropic.rs` (new)

**Interfaces:**
- Consumes: `clax_core::config::HomeConfig::{load, serve_port}` (its `path` and parsed `table`), `CoreError::invalid`.
- Produces:
  - `clax_core::config::{SampleConfig, SampleModels}` (`Deserialize`, `deny_unknown_fields`, defaults as in the Shared contract); `HomeConfig::sample(&self) -> Result<SampleConfig>`: the default when `[sample]` is absent; `Invalid { code: "bad_config" }` naming the file and `[sample]` otherwise invalid.
  - `clax_server::sample::provider::{SampleProvider, EventStream, ProviderRequest, Turn, Role, Block, ToolSpec, ProviderEvent, Stop, ProviderErrorCode}`: `trait SampleProvider: Send + Sync + 'static { fn name(&self) -> &'static str; fn supports_images(&self) -> bool; fn stream(&self, req: ProviderRequest) -> EventStream; }`, `type EventStream = Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>`, `ProviderErrorCode::as_str(self) -> &'static str` (the `sample.d.ts` code).
  - `clax_server::sample::sse::{SseParser, SseEvent}` with `SseParser::push(&mut self, chunk: &[u8]) -> Vec<SseEvent>`.
  - `clax_server::sample::anthropic::{AnthropicProvider, API_VERSION, request_body, Accumulator}`; `AnthropicProvider::new(base_url: impl Into<String>, api_key: impl Into<String>) -> AnthropicProvider`.
  - `clax_server::sample::stub::StubProvider` (`Clone`): `StubProvider::new(images: bool, delay: Duration)`, `StubProvider::requests(&self) -> Vec<ProviderRequest>`; the prompt directives of Step 9.
  - `clax_server::sample::{Sampler, SampleSettings, OffReason}`: `Sampler::disabled()`, `Sampler::new(provider, settings)`, `Sampler::from_config(cfg: &SampleConfig, env: impl Fn(&str) -> Option<String>)`, `Sampler::from_home(home_root: &Path, env)`, `Sampler::{provider, available, provider_name, reason, key_env}`; `OffReason::{NoKey, BadConfig(String), Disabled}`; `SampleSettings { models, max_tokens, daily_call_cap, max_rounds, tool_timeout }` with `Default` and `SampleSettings::from_config(&SampleConfig)`.
  - `ServeConfig.sample: Arc<Sampler>`; `AppState.sample: Arc<Sampler>`.

- [ ] **Step 1: Dependencies**

In `crates/clax-server/Cargo.toml`, replace the optional `reqwest` line under `[dependencies]` with

```toml
reqwest = { workspace = true, features = ["stream", "rustls-tls"] }
base64.workspace = true
```

and set `test-support = ["dep:tempfile"]` (the `reqwest` that `testing.rs` uses is now always there). `rustls-tls` is reqwest 0.12's rustls with the webpki root store: the daemon never reads the system's certificates, which the musl release builds lack.

Run: `cargo build --workspace --all-targets && cargo tree -p clax-server -e normal -i ring`
Expected: builds; `ring` is in the tree. The release matrix (macOS arm64 and x86_64, Linux musl x86_64 and arm64) now builds a TLS stack into the daemon: note it in the task report, so the controller watches the next release run's musl arm64 job (scan N2).

- [ ] **Step 2: Write the failing config tests**

Append to the test module of `crates/clax-core/src/config.rs`:

```rust
    #[test]
    fn an_absent_sample_table_is_the_default() {
        let s = with("[serve]\nport = 7481\n").sample().unwrap();
        assert_eq!(s, SampleConfig::default());
        assert_eq!((s.provider.as_str(), s.api_key_env.as_str(), s.base_url.as_str()), ("anthropic", "ANTHROPIC_API_KEY", "https://api.anthropic.com"));
        assert_eq!((s.models.quick.as_str(), s.models.default.as_str(), s.models.complex.as_str()), ("claude-haiku-4-5-20251001", "claude-sonnet-5-5", "claude-opus-5-5"));
        assert_eq!((s.max_tokens, s.daily_call_cap, s.stub_images, s.stub_delay_ms), (16000, None, false, 40));
    }

    #[test]
    fn sample_keys_override_the_defaults() {
        let s = with("[sample]\nprovider = \"stub\"\ndaily_call_cap = 3\n[sample.models]\nquick = \"q\"\n").sample().unwrap();
        assert_eq!(s.provider, "stub");
        assert_eq!(s.daily_call_cap, Some(3));
        assert_eq!(s.models.quick, "q");
        assert_eq!(s.models.default, "claude-sonnet-5-5");
    }

    #[test]
    fn a_bad_sample_table_is_bad_config_naming_the_file_and_table_and_the_port_still_reads() {
        for t in [
            "[sample]\nprovider = \"openai\"\n",
            "[sample]\nunknown = 1\n",
            "[sample]\nmax_tokens = 0\n",
            "[sample]\ndaily_call_cap = -1\n",
            "[sample.models]\nquick = \"\"\n",
            "[sample.models]\nhuge = \"x\"\n",
            "sample = 3\n",
        ] {
            let c = with(&format!("{t}[serve]\nport = 7481\n"));
            let e = c.sample().unwrap_err();
            assert!(matches!(e, CoreError::Invalid { code: "bad_config", .. }), "{t}: {e}");
            let m = e.to_string();
            assert!(m.contains("config.toml") && m.contains("[sample]"), "{t}: {m}");
            assert_eq!(c.serve_port().unwrap(), Some(7481), "{t}");
        }
    }
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p clax-core config`
Expected: FAIL to compile (`SampleConfig`, `HomeConfig::sample` not found).

- [ ] **Step 4: Implement `HomeConfig::sample`**

In `crates/clax-core/src/config.rs`, change the module doc's last sentence to: "`[sample]` configures the `sample` capability ([`HomeConfig::sample`]); a daemon whose `[sample]` is invalid starts with sample off. Other tables are reserved (spec §5) and ignored." Then add, above the tests:

```rust
use serde::Deserialize;

/// `[sample]`: the `sample` capability's provider, key variable, models, and cap.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SampleConfig {
    /// `anthropic` (default) or `stub` (a deterministic provider for tests and demos).
    pub provider: String,
    /// The environment variable the daemon reads the API key from, at start.
    pub api_key_env: String,
    /// Where the Messages API is; the key is sent nowhere else.
    pub base_url: String,
    pub models: SampleModels,
    /// Most calls per artifact per local day that reach the provider; none when absent.
    pub daily_call_cap: Option<u32>,
    /// `max_tokens` of each provider request.
    pub max_tokens: u32,
    /// The stub reports image support.
    pub stub_images: bool,
    /// The stub's pause before each streamed piece, in milliseconds.
    pub stub_delay_ms: u64,
}

impl Default for SampleConfig {
    fn default() -> Self {
        SampleConfig {
            provider: "anthropic".into(),
            api_key_env: "ANTHROPIC_API_KEY".into(),
            base_url: "https://api.anthropic.com".into(),
            models: SampleModels::default(),
            daily_call_cap: None,
            max_tokens: 16000,
            stub_images: false,
            stub_delay_ms: 40,
        }
    }
}

/// `[sample.models]`: the model ID each `modelTier` maps to.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SampleModels {
    pub quick: String,
    pub default: String,
    pub complex: String,
}

impl Default for SampleModels {
    fn default() -> Self {
        SampleModels {
            quick: "claude-haiku-4-5-20251001".into(),
            default: "claude-sonnet-5-5".into(),
            complex: "claude-opus-5-5".into(),
        }
    }
}
```

and in `impl HomeConfig`:

```rust
    /// `[sample]`: the default when absent.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file and `[sample]` when it
    /// is not a table, has an unknown key or a mistyped value, names a
    /// provider other than `anthropic` or `stub`, sets `max_tokens = 0`, or
    /// leaves a model ID empty. The caller turns sampling off; it never stops
    /// the daemon.
    pub fn sample(&self) -> Result<SampleConfig> {
        let bad = |what: String| {
            CoreError::invalid("bad_config", format!("{}: [sample] {what}", self.path.display()))
        };
        let Some(v) = self.table.get("sample") else {
            return Ok(SampleConfig::default());
        };
        if !v.is_table() {
            return Err(bad(format!("must be a table, not {}", v.type_str())));
        }
        let s: SampleConfig = v.clone().try_into().map_err(|e: toml::de::Error| bad(e.message().to_string()))?;
        if !matches!(s.provider.as_str(), "anthropic" | "stub") {
            return Err(bad(format!("provider is \"anthropic\" or \"stub\", not \"{}\"", s.provider)));
        }
        if s.max_tokens == 0 {
            return Err(bad("max_tokens must be at least 1".into()));
        }
        for (tier, id) in [("quick", &s.models.quick), ("default", &s.models.default), ("complex", &s.models.complex)] {
            if id.trim().is_empty() {
                return Err(bad(format!("models.{tier} is empty")));
            }
        }
        Ok(s)
    }
```

- [ ] **Step 5: Run the config tests to verify they pass**

Run: `cargo test -p clax-core config && cargo clippy -p clax-core --all-targets -- -D warnings`
Expected: PASS, the existing `[serve]` tests included.

- [ ] **Step 6: Write the failing provider unit tests**

Create `crates/clax-server/src/sample/mod.rs` with `pub mod anthropic; pub mod provider; pub mod sse; pub mod stub;` and the `Sampler` tests (Step 10 adds the code above them):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::config::SampleConfig;

    #[test]
    fn a_missing_or_empty_key_disables_the_anthropic_provider() {
        let cfg = SampleConfig::default();
        assert!(!Sampler::from_config(&cfg, |_| None).available());
        assert!(!Sampler::from_config(&cfg, |_| Some(String::new())).available());
        let on = Sampler::from_config(&cfg, |k| (k == "ANTHROPIC_API_KEY").then(|| "sk-test".to_string()));
        assert_eq!(on.provider_name(), Some("anthropic"));
        assert!(on.provider().unwrap().supports_images());
    }

    #[test]
    fn the_stub_needs_no_key_and_reports_images_as_configured() {
        let cfg = SampleConfig { provider: "stub".into(), stub_images: true, ..SampleConfig::default() };
        let s = Sampler::from_config(&cfg, |_| None);
        assert_eq!(s.provider_name(), Some("stub"));
        assert!(s.provider().unwrap().supports_images());
        assert!(Sampler::disabled().provider().is_none());
    }

    #[test]
    fn a_home_config_names_the_provider_and_a_bad_sample_table_turns_sampling_off() {
        let dir = tempfile::tempdir().unwrap();
        let write = |text: &str| std::fs::write(dir.path().join("config.toml"), text).unwrap();
        let load = || Sampler::from_home(dir.path(), |_| None);
        assert_eq!(load().reason(), Some(OffReason::NoKey));
        write("[sample]\nprovider = \"stub\"\n");
        assert_eq!(load().provider_name(), Some("stub"));
        write("[serve]\nport = 7481\n[sample]\nprovider = \"openai\"\n");
        let off = load();
        assert!(!off.available());
        assert!(matches!(off.reason(), Some(OffReason::BadConfig(m)) if m.contains("[sample]")));
        write("[sample\n");
        assert!(matches!(load().reason(), Some(OffReason::BadConfig(_))));
    }

    #[test]
    fn settings_follow_the_config() {
        let cfg = SampleConfig { daily_call_cap: Some(3), max_tokens: 99, ..SampleConfig::default() };
        let s = SampleSettings::from_config(&cfg);
        assert_eq!((s.daily_call_cap, s.max_tokens, s.max_rounds), (Some(3), 99, 5));
        assert_eq!(s.tool_timeout, std::time::Duration::from_secs(150));
    }
}
```

`crates/clax-server/src/sample/sse.rs`, tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_complete_at_a_blank_line_across_any_chunking() {
        let body = b"event: a\ndata: {\"x\":1}\n\n: comment\n\nevent: b\r\ndata: one\r\ndata: two\r\n\r\ndata: plain\n\n";
        for size in [1, 2, 7, body.len()] {
            let mut p = SseParser::default();
            let mut got = Vec::new();
            for chunk in body.chunks(size) {
                got.extend(p.push(chunk));
            }
            assert_eq!(
                got,
                vec![
                    SseEvent { event: "a".into(), data: "{\"x\":1}".into() },
                    SseEvent { event: "b".into(), data: "one\ntwo".into() },
                    SseEvent { event: "message".into(), data: "plain".into() },
                ],
                "chunk size {size}"
            );
        }
    }

    #[test]
    fn multibyte_characters_split_across_chunks_survive() {
        let body = "data: café ☕\n\n".as_bytes();
        let mut p = SseParser::default();
        let mut got = p.push(&body[..9]);
        got.extend(p.push(&body[9..]));
        assert_eq!(got, vec![SseEvent { event: "message".into(), data: "café ☕".into() }]);
    }
}
```

`crates/clax-server/src/sample/anthropic.rs`, tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::provider::*;
    use serde_json::json;

    fn run(acc: &mut Accumulator, events: &[(&str, serde_json::Value)]) -> Vec<ProviderEvent> {
        events.iter().flat_map(|(e, d)| acc.apply(e, &d.to_string())).collect()
    }

    #[test]
    fn text_deltas_stream_and_the_turn_is_kept_verbatim() {
        let mut acc = Accumulator::default();
        let out = run(&mut acc, &[
            ("message_start", json!({"type": "message_start", "message": {"id": "msg_1"}})),
            ("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}})),
            ("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "hmm"}})),
            ("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}})),
            ("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            ("content_block_start", json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}})),
            ("ping", json!({"type": "ping"})),
            ("content_block_delta", json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Hel"}})),
            ("content_block_delta", json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "lo"}})),
            ("content_block_stop", json!({"type": "content_block_stop", "index": 1})),
            ("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}})),
            ("message_stop", json!({"type": "message_stop"})),
        ]);
        assert_eq!(out, vec![
            ProviderEvent::Text("Hel".into()),
            ProviderEvent::Text("lo".into()),
            ProviderEvent::End {
                stop: Stop::EndTurn,
                assistant: vec![
                    json!({"type": "thinking", "thinking": "hmm", "signature": "sig"}),
                    json!({"type": "text", "text": "Hello"}),
                ],
            },
        ]);
    }

    #[test]
    fn tool_input_arrives_in_pieces_and_is_parsed_at_block_stop() {
        let mut acc = Accumulator::default();
        let out = run(&mut acc, &[
            ("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "getColor", "input": {}}})),
            ("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"loc"}})),
            ("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "ation\": \"Paris\"}"}})),
            ("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            ("content_block_start", json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_2", "name": "now", "input": {}}})),
            ("content_block_stop", json!({"type": "content_block_stop", "index": 1})),
            ("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}})),
            ("message_stop", json!({"type": "message_stop"})),
        ]);
        assert_eq!(out[0], ProviderEvent::ToolUse { id: "toolu_1".into(), name: "getColor".into(), input: json!({"location": "Paris"}) });
        assert_eq!(out[1], ProviderEvent::ToolUse { id: "toolu_2".into(), name: "now".into(), input: json!({}) });
        let ProviderEvent::End { stop, assistant } = &out[2] else { panic!("{out:?}") };
        assert_eq!(*stop, Stop::ToolUse);
        assert_eq!(assistant[0]["input"], json!({"location": "Paris"}));
    }

    #[test]
    fn stop_reasons_and_stream_errors_map_to_the_contract() {
        for (reason, stop) in [("max_tokens", Stop::MaxTokens), ("refusal", Stop::Refusal), ("stop_sequence", Stop::EndTurn), ("pause_turn", Stop::EndTurn)] {
            let mut acc = Accumulator::default();
            let out = run(&mut acc, &[
                ("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": reason}})),
                ("message_stop", json!({"type": "message_stop"})),
            ]);
            assert_eq!(out, vec![ProviderEvent::End { stop, assistant: vec![] }], "{reason}");
        }
        let mut acc = Accumulator::default();
        let out = run(&mut acc, &[("error", json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}))]);
        assert_eq!(out, vec![ProviderEvent::Error { code: ProviderErrorCode::Upstream, message: "Overloaded".into() }]);
    }

    #[test]
    fn http_errors_map_by_type_then_status() {
        let e = |status: u16, body: &str| match http_error(status, body, "sk-secret") {
            ProviderEvent::Error { code, .. } => code,
            other => panic!("{other:?}"),
        };
        let typed = |t: &str, m: &str| json!({"type": "error", "error": {"type": t, "message": m}}).to_string();
        assert_eq!(e(429, &typed("rate_limit_error", "slow down")), ProviderErrorCode::RateLimited);
        assert_eq!(e(401, &typed("authentication_error", "bad key")), ProviderErrorCode::Unavailable);
        assert_eq!(e(402, &typed("billing_error", "no credit")), ProviderErrorCode::Unavailable);
        assert_eq!(e(404, &typed("not_found_error", "model: x")), ProviderErrorCode::Unavailable);
        assert_eq!(e(400, &typed("invalid_request_error", "prompt is too long: 300000 tokens > 200000 maximum")), ProviderErrorCode::PromptTooLarge);
        assert_eq!(e(400, &typed("invalid_request_error", "tools.0.input_schema: invalid")), ProviderErrorCode::InvalidRequest);
        assert_eq!(e(413, &typed("request_too_large", "too big")), ProviderErrorCode::PromptTooLarge);
        assert_eq!(e(529, &typed("overloaded_error", "busy")), ProviderErrorCode::Upstream);
        assert_eq!(e(503, "<html>"), ProviderErrorCode::Upstream);
        assert_eq!(e(429, ""), ProviderErrorCode::RateLimited);
    }

    #[test]
    fn provider_errors_never_carry_the_key() {
        let body = json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key: sk-secret"}}).to_string();
        let ProviderEvent::Error { message, .. } = http_error(401, &body, "sk-secret") else { panic!() };
        assert!(!message.contains("sk-secret"), "{message}");
        assert!(message.contains("[redacted]"), "{message}");
    }

    #[test]
    fn the_provider_debug_string_hides_the_key() {
        let p = AnthropicProvider::new("http://127.0.0.1:9", "sk-secret");
        assert!(!format!("{p:?}").contains("sk-secret"));
    }

    #[test]
    fn the_request_body_echoes_raw_turns_and_ends_tool_rounds_with_tool_choice_none() {
        let req = ProviderRequest {
            model: "claude-sonnet-5-5".into(),
            max_tokens: 100,
            system: "frame".into(),
            messages: vec![
                Turn { role: Role::User, content: vec![Block::Image { media_type: "image/png".into(), data: "AAAA".into() }, Block::Text("hi".into())] },
                Turn { role: Role::Assistant, content: vec![Block::Raw(json!({"type": "thinking", "thinking": "", "signature": "s"})), Block::Raw(json!({"type": "tool_use", "id": "t1", "name": "n", "input": {}}))] },
                Turn { role: Role::User, content: vec![Block::ToolResult { tool_use_id: "t1".into(), content: "teal".into(), is_error: false }] },
            ],
            tools: vec![ToolSpec { name: "n".into(), description: "d".into(), input_schema: json!({"type": "object", "properties": {}}) }],
            final_round: true,
        };
        assert_eq!(request_body(&req), json!({
            "model": "claude-sonnet-5-5", "max_tokens": 100, "stream": true, "system": "frame",
            "messages": [
                {"role": "user", "content": [
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}},
                    {"type": "text", "text": "hi"}]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "", "signature": "s"},
                    {"type": "tool_use", "id": "t1", "name": "n", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "teal", "is_error": false}]}],
            "tools": [{"name": "n", "description": "d", "input_schema": {"type": "object", "properties": {}}}],
            "tool_choice": {"type": "none"}
        }));
        let no_tools = ProviderRequest { tools: vec![], final_round: false, ..req };
        let body = request_body(&no_tools);
        assert!(body.get("tools").is_none() && body.get("tool_choice").is_none());
    }
}
```

`crates/clax-server/src/sample/stub.rs`, tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;

    fn req(text: &str) -> ProviderRequest {
        ProviderRequest {
            model: "m".into(),
            max_tokens: 10,
            system: "s".into(),
            messages: vec![Turn { role: Role::User, content: vec![Block::Text(text.into())] }],
            tools: vec![],
            final_round: true,
        }
    }

    async fn all(p: &StubProvider, r: ProviderRequest) -> Vec<ProviderEvent> {
        p.stream(r).collect().await
    }

    fn text(evs: &[ProviderEvent]) -> String {
        evs.iter().filter_map(|e| if let ProviderEvent::Text(t) = e { Some(t.as_str()) } else { None }).collect()
    }

    #[tokio::test]
    async fn echoes_in_three_pieces_and_records_the_request() {
        let p = StubProvider::new(false, Duration::ZERO);
        let evs = all(&p, req("hello there")).await;
        assert_eq!(evs.iter().filter(|e| matches!(e, ProviderEvent::Text(_))).count(), 3);
        assert_eq!(text(&evs), "echo: hello there");
        assert!(matches!(evs.last(), Some(ProviderEvent::End { stop: Stop::EndTurn, .. })));
        assert_eq!(p.requests().len(), 1);
    }

    #[tokio::test]
    async fn scripts() {
        let p = StubProvider::new(false, Duration::ZERO);
        let evs = all(&p, req("x [[say:{\"a\": 1}]]")).await;
        assert_eq!(text(&evs), "{\"a\": 1}");
        let evs = all(&p, req("[[refuse]]")).await;
        assert!(matches!(evs.last(), Some(ProviderEvent::End { stop: Stop::Refusal, .. })));
        let evs = all(&p, req("[[empty]]")).await;
        assert_eq!(text(&evs), "");
        let evs = all(&p, req("[[truncate]]")).await;
        assert!(matches!(evs.last(), Some(ProviderEvent::End { stop: Stop::MaxTokens, .. })));
        let evs = all(&p, req("[[error:rate_limited]]")).await;
        assert_eq!(evs, vec![ProviderEvent::Error { code: ProviderErrorCode::RateLimited, message: "stub error".into() }]);
        let evs = all(&p, req("[[error-after:upstream_error]]")).await;
        assert_eq!(text(&evs), "partial ");
        assert!(matches!(evs.last(), Some(ProviderEvent::Error { code: ProviderErrorCode::Upstream, .. })));
    }

    #[tokio::test]
    async fn a_tool_directive_calls_the_tool_then_answers_from_its_result() {
        let p = StubProvider::new(false, Duration::ZERO);
        let first = ProviderRequest { final_round: false, ..req("[[tool:getColor {\"shade\": \"dark\"}]]") };
        let evs = all(&p, first.clone()).await;
        assert_eq!(text(&evs), "Checking.");
        assert!(evs.contains(&ProviderEvent::ToolUse { id: "toolu_stub_1".into(), name: "getColor".into(), input: json!({"shade": "dark"}) }));
        let ProviderEvent::End { stop: Stop::ToolUse, assistant } = evs.last().unwrap() else { panic!("{evs:?}") };
        let mut second = first.clone();
        second.messages.push(Turn { role: Role::Assistant, content: assistant.iter().cloned().map(Block::Raw).collect() });
        second.messages.push(Turn { role: Role::User, content: vec![Block::ToolResult { tool_use_id: "toolu_stub_1".into(), content: "teal".into(), is_error: false }] });
        assert_eq!(text(&all(&p, second).await), "tool said: teal");
        let last = ProviderRequest { final_round: true, ..req("[[tool:getColor]]") };
        assert_eq!(text(&all(&p, last).await), "no rounds left");
    }

    #[tokio::test]
    async fn counts_the_images_it_was_shown() {
        let p = StubProvider::new(true, Duration::ZERO);
        let mut r = req("[[images]]");
        r.messages[0].content.insert(0, Block::Image { media_type: "image/png".into(), data: "AAAA".into() });
        assert_eq!(text(&all(&p, r).await), "saw 1 images");
        assert!(p.supports_images());
    }
}
```

- [ ] **Step 7: Run the unit tests to verify they fail**

Run: `cargo test -p clax-server --lib sample`
Expected: FAIL to compile.

- [ ] **Step 8: Implement the provider types and the SSE parser**

`crates/clax-server/src/sample/provider.rs`:

```rust
//! The provider behind `sample()` (spec D10: "provider is a trait"). A
//! provider turns one [`ProviderRequest`] (one round) into a stream of
//! [`ProviderEvent`]s ending in exactly one `End` or `Error`. Dropping the
//! stream cancels the request. The orchestration of rounds, page tools,
//! caching, and counting is not the provider's (see `sample::flight`).

use futures::Stream;
use serde_json::Value;
use std::pin::Pin;

pub type EventStream = Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>;

pub trait SampleProvider: Send + Sync + 'static {
    /// `anthropic` or `stub`.
    fn name(&self) -> &'static str;
    /// Whether requests may carry [`Block::Image`].
    fn supports_images(&self) -> bool;
    /// Starts one round.
    fn stream(&self, req: ProviderRequest) -> EventStream;
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderRequest {
    pub model: String,
    pub max_tokens: u32,
    /// The fixed framing (`sample::request::FRAMING`); pages never set it.
    pub system: String,
    pub messages: Vec<Turn>,
    pub tools: Vec<ToolSpec>,
    /// The last round of a call with tools: the model must answer, not call tools.
    pub final_round: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub role: Role,
    pub content: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Text(String),
    /// Base64 image bytes.
    Image { media_type: String, data: String },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    /// One block of an earlier assistant turn exactly as the same provider
    /// produced it (`ProviderEvent::End.assistant`), passed back unchanged.
    Raw(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderEvent {
    Text(String),
    ToolUse { id: String, name: String, input: Value },
    /// The round ended; `assistant` is the round's content blocks, to be
    /// passed back as [`Block::Raw`] if another round follows.
    End { stop: Stop, assistant: Vec<Value> },
    Error { code: ProviderErrorCode, message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    EndTurn,
    MaxTokens,
    ToolUse,
    Refusal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderErrorCode {
    RateLimited,
    /// The key, the account, or the model cannot be used.
    Unavailable,
    InvalidRequest,
    PromptTooLarge,
    Upstream,
}

impl ProviderErrorCode {
    /// The `sample.d.ts` error code a page sees.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderErrorCode::RateLimited => "rate_limited",
            ProviderErrorCode::Unavailable => "sampling_disabled",
            ProviderErrorCode::InvalidRequest => "invalid_request",
            ProviderErrorCode::PromptTooLarge => "prompt_too_large",
            ProviderErrorCode::Upstream => "upstream_error",
        }
    }

    /// Parses the codes the stub's `[[error:<code>]]` accepts; anything else is `Upstream`.
    pub fn parse(code: &str) -> ProviderErrorCode {
        match code {
            "rate_limited" => ProviderErrorCode::RateLimited,
            "sampling_disabled" | "unavailable" => ProviderErrorCode::Unavailable,
            "invalid_request" => ProviderErrorCode::InvalidRequest,
            "prompt_too_large" => ProviderErrorCode::PromptTooLarge,
            _ => ProviderErrorCode::Upstream,
        }
    }
}
```

`crates/clax-server/src/sample/sse.rs`, above its tests:

```rust
//! An incremental `text/event-stream` parser: feed it bytes in any chunking,
//! get whole events back. `event:` names the event (default `message`),
//! `data:` lines are joined by `\n`, comments and other fields are ignored.

#[derive(Clone, Debug, PartialEq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

/// The end of the first event in `b` and the length of its blank-line separator.
fn blank_line(b: &[u8]) -> Option<(usize, usize)> {
    (0..b.len()).find_map(|i| {
        if b[i..].starts_with(b"\r\n\r\n") {
            Some((i, 4))
        } else if b[i..].starts_with(b"\n\n") {
            Some((i, 2))
        } else {
            None
        }
    })
}

impl SseParser {
    /// Adds `chunk` and returns every event it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some((end, sep)) = blank_line(&self.buf) {
            let block: Vec<u8> = self.buf.drain(..end + sep).collect();
            let text = String::from_utf8_lossy(&block[..end]);
            let mut event = String::from("message");
            let mut data: Vec<&str> = Vec::new();
            for line in text.split('\n') {
                let line = line.strip_suffix('\r').unwrap_or(line);
                if let Some(v) = line.strip_prefix("event:") {
                    event = v.trim_start().to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push(v.strip_prefix(' ').unwrap_or(v));
                }
            }
            if !data.is_empty() {
                out.push(SseEvent { event, data: data.join("\n") });
            }
        }
        out
    }
}
```

- [ ] **Step 9: Implement the Anthropic provider and the stub**

`crates/clax-server/src/sample/anthropic.rs`, above its tests:

```rust
//! The Messages API provider (spec D10): one streaming `POST {base_url}/v1/messages`
//! per round, with the key in `x-api-key` and nowhere else. Assistant content
//! blocks (thinking blocks with their signatures included) are collected as
//! they stream and handed back whole in `ProviderEvent::End`, so the next tool
//! round can pass them back unchanged. Provider error messages have the key
//! replaced by `[redacted]`.

use super::provider::*;
use super::sse::SseParser;
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

pub const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider").field("base_url", &self.base_url).finish_non_exhaustive()
    }
}

impl AnthropicProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> AnthropicProvider {
        AnthropicProvider {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("a reqwest client with default TLS"),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }
}

fn block_json(b: &Block) -> Value {
    match b {
        Block::Text(t) => json!({"type": "text", "text": t}),
        Block::Image { media_type, data } => {
            json!({"type": "image", "source": {"type": "base64", "media_type": media_type, "data": data}})
        }
        Block::ToolResult { tool_use_id, content, is_error } => {
            json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": is_error})
        }
        Block::Raw(v) => v.clone(),
    }
}

/// The JSON body of one round's request.
pub fn request_body(req: &ProviderRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|t| json!({"role": t.role.as_str(), "content": t.content.iter().map(block_json).collect::<Vec<_>>()}))
        .collect();
    let mut body = json!({
        "model": req.model, "max_tokens": req.max_tokens, "stream": true,
        "system": req.system, "messages": messages,
    });
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
            .collect();
        if req.final_round {
            body["tool_choice"] = json!({"type": "none"});
        }
    }
    body
}

fn redact(message: &str, key: &str) -> String {
    if key.is_empty() { message.to_string() } else { message.replace(key, "[redacted]") }
}

fn error_code(kind: &str, message: &str) -> ProviderErrorCode {
    match kind {
        "rate_limit_error" => ProviderErrorCode::RateLimited,
        "authentication_error" | "permission_error" | "billing_error" | "not_found_error" => ProviderErrorCode::Unavailable,
        "request_too_large" => ProviderErrorCode::PromptTooLarge,
        "invalid_request_error" if message.contains("prompt is too long") => ProviderErrorCode::PromptTooLarge,
        "invalid_request_error" => ProviderErrorCode::InvalidRequest,
        _ => ProviderErrorCode::Upstream,
    }
}

/// The event for a non-2xx answer: by the body's error type when it has one, else by status.
pub fn http_error(status: u16, body: &str, key: &str) -> ProviderEvent {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let err = parsed.as_ref().map(|v| &v["error"]).filter(|e| e.is_object());
    let (code, message) = match err {
        Some(e) => {
            let message = e["message"].as_str().unwrap_or("").to_string();
            (error_code(e["type"].as_str().unwrap_or(""), &message), message)
        }
        None => (
            match status {
                429 => ProviderErrorCode::RateLimited,
                401..=404 => ProviderErrorCode::Unavailable,
                413 => ProviderErrorCode::PromptTooLarge,
                400 => ProviderErrorCode::InvalidRequest,
                _ => ProviderErrorCode::Upstream,
            },
            format!("the provider answered HTTP {status}"),
        ),
    };
    ProviderEvent::Error { code, message: redact(&message, key) }
}

fn stop_from(reason: &str) -> Stop {
    match reason {
        "max_tokens" => Stop::MaxTokens,
        "tool_use" => Stop::ToolUse,
        "refusal" => Stop::Refusal,
        _ => Stop::EndTurn,
    }
}

/// Folds one round's stream events into provider events and the round's content blocks.
#[derive(Default)]
pub struct Accumulator {
    blocks: Vec<Value>,
    partial: HashMap<usize, String>,
    stop: Option<Stop>,
}

impl Accumulator {
    fn block(&mut self, i: usize) -> &mut Value {
        if self.blocks.len() <= i {
            self.blocks.resize(i + 1, Value::Null);
        }
        &mut self.blocks[i]
    }

    fn append(&mut self, i: usize, field: &str, piece: &str) {
        let b = self.block(i);
        let now = format!("{}{piece}", b[field].as_str().unwrap_or(""));
        b[field] = Value::String(now);
    }

    /// Applies one SSE event; returns what it produced for the orchestrator.
    pub fn apply(&mut self, event: &str, data: &str) -> Vec<ProviderEvent> {
        let Ok(v) = serde_json::from_str::<Value>(data) else { return Vec::new() };
        let index = v["index"].as_u64().unwrap_or(0) as usize;
        match v["type"].as_str().unwrap_or(event) {
            "content_block_start" => {
                let mut b = v["content_block"].clone();
                if b["type"] == "tool_use" {
                    b["input"] = json!({});
                }
                *self.block(index) = b;
                Vec::new()
            }
            "content_block_delta" => {
                let d = &v["delta"];
                match d["type"].as_str() {
                    Some("text_delta") => {
                        let t = d["text"].as_str().unwrap_or("").to_string();
                        self.append(index, "text", &t);
                        vec![ProviderEvent::Text(t)]
                    }
                    Some("thinking_delta") => {
                        let t = d["thinking"].as_str().unwrap_or("").to_string();
                        self.append(index, "thinking", &t);
                        Vec::new()
                    }
                    Some("signature_delta") => {
                        let sig = d["signature"].clone();
                        self.block(index)["signature"] = sig;
                        Vec::new()
                    }
                    Some("input_json_delta") => {
                        self.partial.entry(index).or_default().push_str(d["partial_json"].as_str().unwrap_or(""));
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            "content_block_stop" => {
                if self.block(index)["type"] != "tool_use" {
                    return Vec::new();
                }
                let raw = self.partial.remove(&index).unwrap_or_default();
                let input = if raw.trim().is_empty() { json!({}) } else { serde_json::from_str(&raw).unwrap_or_else(|_| json!({})) };
                let b = self.block(index);
                b["input"] = input.clone();
                vec![ProviderEvent::ToolUse {
                    id: b["id"].as_str().unwrap_or("").to_string(),
                    name: b["name"].as_str().unwrap_or("").to_string(),
                    input,
                }]
            }
            "message_delta" => {
                if let Some(r) = v["delta"]["stop_reason"].as_str() {
                    self.stop = Some(stop_from(r));
                }
                Vec::new()
            }
            "message_stop" => vec![ProviderEvent::End {
                stop: self.stop.unwrap_or(Stop::EndTurn),
                assistant: std::mem::take(&mut self.blocks).into_iter().filter(|b| !b.is_null()).collect(),
            }],
            "error" => {
                let e = &v["error"];
                let message = e["message"].as_str().unwrap_or("the provider reported an error").to_string();
                vec![ProviderEvent::Error { code: error_code(e["type"].as_str().unwrap_or(""), &message), message }]
            }
            _ => Vec::new(),
        }
    }
}

async fn run(client: reqwest::Client, url: String, key: String, req: ProviderRequest, tx: mpsc::Sender<ProviderEvent>) {
    let upstream = |m: String| ProviderEvent::Error { code: ProviderErrorCode::Upstream, message: m };
    let res = match client
        .post(&url)
        .header("x-api-key", &key)
        .header("anthropic-version", API_VERSION)
        .json(&request_body(&req))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(upstream(redact(&format!("could not reach the provider: {e}"), &key))).await;
            return;
        }
    };
    let status = res.status().as_u16();
    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        let _ = tx.send(http_error(status, &body, &key)).await;
        return;
    }
    let mut parser = SseParser::default();
    let mut acc = Accumulator::default();
    let mut bytes = res.bytes_stream();
    while let Some(chunk) = bytes.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(upstream(redact(&format!("the provider's stream broke: {e}"), &key))).await;
                return;
            }
        };
        for ev in parser.push(&chunk) {
            for out in acc.apply(&ev.event, &ev.data) {
                let last = matches!(out, ProviderEvent::End { .. } | ProviderEvent::Error { .. });
                let out = match out {
                    ProviderEvent::Error { code, message } => ProviderEvent::Error { code, message: redact(&message, &key) },
                    other => other,
                };
                if tx.send(out).await.is_err() || last {
                    return;
                }
            }
        }
    }
    let _ = tx.send(upstream("the provider's stream ended before message_stop".into())).await;
}

impl super::provider::SampleProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn supports_images(&self) -> bool {
        true
    }

    fn stream(&self, req: ProviderRequest) -> EventStream {
        let (tx, rx) = mpsc::channel(64);
        let (client, url, key) = (self.client.clone(), format!("{}/v1/messages", self.base_url), self.api_key.clone());
        tokio::spawn(async move {
            let closed = tx.clone();
            tokio::select! {
                () = closed.closed() => {}
                () = run(client, url, key, req, tx) => {}
            }
        });
        Box::pin(ReceiverStream::new(rx))
    }
}
```

`crates/clax-server/src/sample/stub.rs`, above its tests:

```rust
//! A deterministic provider for tests and demos (`[sample] provider = "stub"`).
//! It answers from directives in the latest user text and never makes a
//! network call:
//!
//! - default: `echo: <text>` in three pieces;
//! - `[[say:TEXT]]`: exactly TEXT (everything up to the last `]]`);
//! - `[[tool:NAME]]` or `[[tool:NAME {json}]]`: `Checking.`, then a call of
//!   page tool NAME with that input (`{}` when absent); on the last round,
//!   `no rounds left`;
//! - a turn of tool results: `tool said: <results joined by "; ">`;
//! - `[[slow]]`: `tick ` fifty times, 100 ms apart;
//! - `[[refuse]]`: `I won` then a refusal; `[[empty]]`: no text;
//!   `[[truncate]]`: `cut` then the length limit;
//! - `[[error:CODE]]`: an error with that code (`ProviderErrorCode::parse`);
//!   `[[error-after:CODE]]`: `partial ` then that error;
//! - `[[images]]`: `saw N images` for the image blocks of the latest user turn.
//!
//! Each text piece waits `delay` first (`stub_delay_ms`). Every request is
//! recorded for tests.

use super::provider::*;
use futures::StreamExt;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct StubProvider {
    images: bool,
    delay: Duration,
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
}

impl StubProvider {
    pub fn new(images: bool, delay: Duration) -> StubProvider {
        StubProvider { images, delay, requests: Arc::default() }
    }

    /// Every request this provider has been sent, oldest first.
    pub fn requests(&self) -> Vec<ProviderRequest> {
        self.requests.lock().expect("stub requests lock").clone()
    }
}

fn latest_user_text(req: &ProviderRequest) -> String {
    req.messages
        .iter()
        .rev()
        .filter(|t| t.role == Role::User)
        .find_map(|t| t.content.iter().rev().find_map(|b| if let Block::Text(s) = b { Some(s.clone()) } else { None }))
        .unwrap_or_default()
}

fn end(stop: Stop, text: &str, extra: Vec<Value>) -> ProviderEvent {
    let mut assistant = Vec::new();
    if !text.is_empty() {
        assistant.push(json!({"type": "text", "text": text}));
    }
    assistant.extend(extra);
    ProviderEvent::End { stop, assistant }
}

fn directive<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let start = text.find(&format!("[[{name}"))? + 2 + name.len();
    let end = text.rfind("]]")?;
    (end >= start).then(|| &text[start..end])
}

/// The events of one round, each with the pause before it.
fn script(req: &ProviderRequest, delay: Duration) -> Vec<(Duration, ProviderEvent)> {
    use ProviderEvent::{Error, Text, ToolUse};
    let z = Duration::ZERO;
    let results: Vec<&str> = req
        .messages
        .last()
        .map(|t| t.content.iter().filter_map(|b| if let Block::ToolResult { content, .. } = b { Some(content.as_str()) } else { None }).collect())
        .unwrap_or_default();
    if !results.is_empty() {
        let t = format!("tool said: {}", results.join("; "));
        return vec![(delay, Text(t.clone())), (z, end(Stop::EndTurn, &t, vec![]))];
    }
    let prompt = latest_user_text(req);
    if let Some(say) = directive(&prompt, "say:") {
        return vec![(delay, Text(say.to_string())), (z, end(Stop::EndTurn, say, vec![]))];
    }
    if let Some(spec) = directive(&prompt, "tool:") {
        if req.final_round {
            return vec![(delay, Text("no rounds left".into())), (z, end(Stop::EndTurn, "no rounds left", vec![]))];
        }
        let (name, input) = match spec.split_once(' ') {
            Some((n, j)) => (n.to_string(), serde_json::from_str(j.trim()).unwrap_or_else(|_| json!({}))),
            None => (spec.trim().to_string(), json!({})),
        };
        let call = json!({"type": "tool_use", "id": "toolu_stub_1", "name": name, "input": input});
        return vec![
            (delay, Text("Checking.".into())),
            (z, ToolUse { id: "toolu_stub_1".into(), name, input }),
            (z, end(Stop::ToolUse, "Checking.", vec![call])),
        ];
    }
    if prompt.contains("[[slow]]") {
        let mut out: Vec<(Duration, ProviderEvent)> = (0..50).map(|_| (Duration::from_millis(100), Text("tick ".into()))).collect();
        out.push((z, end(Stop::EndTurn, &"tick ".repeat(50), vec![])));
        return out;
    }
    if prompt.contains("[[refuse]]") {
        return vec![(delay, Text("I won".into())), (z, end(Stop::Refusal, "I won", vec![]))];
    }
    if prompt.contains("[[empty]]") {
        return vec![(z, end(Stop::EndTurn, "", vec![]))];
    }
    if prompt.contains("[[truncate]]") {
        return vec![(delay, Text("cut".into())), (z, end(Stop::MaxTokens, "cut", vec![]))];
    }
    if let Some(code) = directive(&prompt, "error-after:") {
        return vec![(delay, Text("partial ".into())), (z, Error { code: ProviderErrorCode::parse(code), message: "stub error".into() })];
    }
    if let Some(code) = directive(&prompt, "error:") {
        return vec![(z, Error { code: ProviderErrorCode::parse(code), message: "stub error".into() })];
    }
    if prompt.contains("[[images]]") {
        let n = req
            .messages
            .iter()
            .rev()
            .find(|t| t.role == Role::User)
            .map_or(0, |t| t.content.iter().filter(|b| matches!(b, Block::Image { .. })).count());
        let t = format!("saw {n} images");
        return vec![(delay, Text(t.clone())), (z, end(Stop::EndTurn, &t, vec![]))];
    }
    let whole = format!("echo: {prompt}");
    let chars: Vec<char> = whole.chars().collect();
    let size = chars.len().div_ceil(3).max(1);
    let mut out: Vec<(Duration, ProviderEvent)> = chars.chunks(size).map(|c| (delay, Text(c.iter().collect()))).collect();
    out.push((z, end(Stop::EndTurn, &whole, vec![])));
    out
}

impl SampleProvider for StubProvider {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn supports_images(&self) -> bool {
        self.images
    }

    fn stream(&self, req: ProviderRequest) -> EventStream {
        let steps = script(&req, self.delay);
        self.requests.lock().expect("stub requests lock").push(req);
        Box::pin(futures::stream::iter(steps).then(|(pause, ev)| async move {
            if !pause.is_zero() {
                tokio::time::sleep(pause).await;
            }
            ev
        }))
    }
}
```

Directive parsing uses `rfind("]]")`, so `[[tool:NAME {json}]]` and `[[say:...]]` must be the last directive in the prompt; the tests and pages use one directive per prompt.

- [ ] **Step 10: Implement the sampler and wire it into the daemon**

Above the tests in `crates/clax-server/src/sample/mod.rs`:

```rust
//! The `sample` capability's daemon side (spec §6 "Sample", §9, D10): the
//! configured provider and settings. `Sampler::disabled()` has no provider:
//! pages then resolve `use("sample")` to `null` and the routes answer
//! `sampling_disabled`.

pub mod anthropic;
pub mod provider;
pub mod sse;
pub mod stub;

use clax_core::config::{HomeConfig, SampleConfig, SampleModels};
use provider::SampleProvider;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct SampleSettings {
    pub models: SampleModels,
    pub max_tokens: u32,
    pub daily_call_cap: Option<u32>,
    /// Most provider rounds of one call with tools; the last must answer.
    pub max_rounds: usize,
    /// How long a round waits for a page tool's result before sending an error result.
    pub tool_timeout: Duration,
}

impl Default for SampleSettings {
    fn default() -> Self {
        SampleSettings::from_config(&SampleConfig::default())
    }
}

impl SampleSettings {
    pub fn from_config(cfg: &SampleConfig) -> SampleSettings {
        SampleSettings {
            models: cfg.models.clone(),
            max_tokens: cfg.max_tokens,
            daily_call_cap: cfg.daily_call_cap,
            max_rounds: 5,
            tool_timeout: Duration::from_secs(150),
        }
    }
}

/// Why sampling is off on this daemon (`clax doctor` and `GET /api/sample` report it).
#[derive(Clone, Debug, PartialEq)]
pub enum OffReason {
    /// The `anthropic` provider's key variable is unset or empty.
    NoKey,
    /// `config.toml` or its `[sample]` table is invalid; the message names the problem.
    BadConfig(String),
    /// Built with [`Sampler::disabled`] (tests).
    Disabled,
}

pub struct Sampler {
    provider: Option<Arc<dyn SampleProvider>>,
    reason: Option<OffReason>,
    /// The key variable the provider reads (named in reports, never its value).
    key_env: Option<String>,
    pub settings: SampleSettings,
}

impl Sampler {
    /// Every constructor ends here, so a field added later (the pending tool
    /// calls, the cache, the counts) is initialised in one place.
    fn build(provider: Option<Arc<dyn SampleProvider>>, reason: Option<OffReason>, key_env: Option<String>, settings: SampleSettings) -> Sampler {
        Sampler { provider, reason, key_env, settings }
    }

    pub fn disabled() -> Sampler {
        Sampler::build(None, Some(OffReason::Disabled), None, SampleSettings::default())
    }

    pub fn new(provider: Arc<dyn SampleProvider>, settings: SampleSettings) -> Sampler {
        Sampler::build(Some(provider), None, None, settings)
    }

    /// The provider `cfg` names. `anthropic` needs a non-empty key in the
    /// variable `cfg.api_key_env` (read through `env`), else sampling is off.
    pub fn from_config(cfg: &SampleConfig, env: impl Fn(&str) -> Option<String>) -> Sampler {
        let settings = SampleSettings::from_config(cfg);
        if cfg.provider == "stub" {
            let stub = stub::StubProvider::new(cfg.stub_images, Duration::from_millis(cfg.stub_delay_ms));
            return Sampler::build(Some(Arc::new(stub)), None, None, settings);
        }
        let key_env = Some(cfg.api_key_env.clone());
        match env(&cfg.api_key_env).filter(|k| !k.trim().is_empty()) {
            Some(k) => {
                let p = anthropic::AnthropicProvider::new(cfg.base_url.clone(), k);
                Sampler::build(Some(Arc::new(p)), None, key_env, settings)
            }
            None => Sampler::build(None, Some(OffReason::NoKey), key_env, settings),
        }
    }

    /// The home's `config.toml` `[sample]` table ([`HomeConfig::sample`]),
    /// read once when the daemon starts. A file or table that is invalid
    /// turns sampling off, logs one warning, and never stops the daemon.
    pub fn from_home(home_root: &std::path::Path, env: impl Fn(&str) -> Option<String>) -> Sampler {
        match HomeConfig::load(home_root).and_then(|c| c.sample()) {
            Ok(cfg) => Sampler::from_config(&cfg, env),
            Err(e) => {
                tracing::warn!(error = %e, "sample() is off");
                Sampler::build(None, Some(OffReason::BadConfig(e.to_string())), None, SampleSettings::default())
            }
        }
    }

    /// Why sampling is off; `None` when a provider is configured.
    pub fn reason(&self) -> Option<OffReason> {
        self.reason.clone()
    }

    /// The variable the `anthropic` provider reads its key from (its name only).
    pub fn key_env(&self) -> Option<&str> {
        self.key_env.as_deref()
    }

    pub fn provider(&self) -> Option<&Arc<dyn SampleProvider>> {
        self.provider.as_ref()
    }

    pub fn available(&self) -> bool {
        self.provider.is_some()
    }

    pub fn provider_name(&self) -> Option<&'static str> {
        self.provider.as_ref().map(|p| p.name())
    }
}
```

In `crates/clax-server/src/lib.rs` add `pub mod sample;`. In `state.rs`:

```rust
    /// The `sample` capability's provider and settings.
    pub sample: std::sync::Arc<crate::sample::Sampler>,
```

In `daemon.rs`, `ServeConfig` gains the sampler the caller built, beside `codex`:

```rust
    /// The `sample` capability's provider and settings (`clax serve` uses
    /// [`crate::sample::Sampler::from_home`] on its own environment).
    pub sample: std::sync::Arc<crate::sample::Sampler>,
```

and `AppState` takes it: `sample: cfg.sample.clone(),`, followed by

```rust
    tracing::info!(provider = ?state.sample.provider_name(), "sample provider");
```

beside the existing `codex push` log line. In `crates/clax-cli/src/commands/serve.rs` (wherever `ServeConfig` is built, next to `CodexPush::from_env`), pass

```rust
        sample: std::sync::Arc::new(clax_server::sample::Sampler::from_home(home.root(), |k| std::env::var(k).ok())),
```

Every other `ServeConfig { .. }` in the workspace (the daemon tests in `crates/clax-server/tests/daemon.rs`, any in `clax-cli`) passes `sample: Arc::new(Sampler::disabled())`; `cargo build --workspace --all-targets` lists them. In `testing.rs`: `sample: Arc::new(crate::sample::Sampler::disabled()),` (tests that sample set their own with `spawn_with`).

- [ ] **Step 11: Run the unit tests to verify they pass**

Run: `cargo test -p clax-server --lib sample && cargo clippy -p clax-server --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 12: Write the mocked-server tests**

`crates/clax-server/tests/sample_anthropic.rs`:

```rust
//! The Anthropic provider against a mocked Messages API on 127.0.0.1. No test
//! here reaches a real provider.

use clax_server::sample::anthropic::AnthropicProvider;
use clax_server::sample::provider::*;
use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use futures::StreamExt;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<(HeaderMap, Value)>>>;

/// A mock answering every `POST /v1/messages` with `status` and `body`.
async fn mock(status: u16, body: String) -> (String, Seen) {
    let seen: Seen = Arc::default();
    let s = seen.clone();
    let app = Router::new().route(
        "/v1/messages",
        post(move |headers: HeaderMap, axum::Json(v): axum::Json<Value>| {
            let s = s.clone();
            let body = body.clone();
            async move {
                s.lock().unwrap().push((headers, v));
                (StatusCode::from_u16(status).unwrap(), [("content-type", "text/event-stream")], body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, seen)
}

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())).collect()
}

fn req(text: &str) -> ProviderRequest {
    ProviderRequest {
        model: "claude-sonnet-5-5".into(),
        max_tokens: 64,
        system: "frame".into(),
        messages: vec![Turn { role: Role::User, content: vec![Block::Text(text.into())] }],
        tools: vec![],
        final_round: true,
    }
}

async fn run(base: &str, key: &str, r: ProviderRequest) -> Vec<ProviderEvent> {
    AnthropicProvider::new(base, key).stream(r).collect().await
}

fn text_turn(parts: &[&str], stop: &str) -> String {
    let mut ev = vec![
        json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
    ];
    for p in parts {
        ev.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": p}}));
    }
    ev.push(json!({"type": "content_block_stop", "index": 0}));
    ev.push(json!({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": 3}}));
    ev.push(json!({"type": "message_stop"}));
    sse(&ev)
}

#[tokio::test]
async fn streams_text_with_the_key_and_version_headers() {
    let (base, seen) = mock(200, text_turn(&["Hel", "lo"], "end_turn")).await;
    let out = run(&base, "sk-test", req("hi")).await;
    assert_eq!(out[..2], [ProviderEvent::Text("Hel".into()), ProviderEvent::Text("lo".into())]);
    assert_eq!(out[2], ProviderEvent::End { stop: Stop::EndTurn, assistant: vec![json!({"type": "text", "text": "Hello"})] });
    let (headers, body) = seen.lock().unwrap()[0].clone();
    assert_eq!(headers["x-api-key"], "sk-test");
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    assert_eq!(body["stream"], true);
    assert_eq!(body["model"], "claude-sonnet-5-5");
    assert_eq!(body["system"], "frame");
    assert_eq!(body["messages"], json!([{"role": "user", "content": [{"type": "text", "text": "hi"}]}]));
}

#[tokio::test]
async fn tool_calls_arrive_parsed_with_the_turn_to_echo() {
    let body = sse(&[
        json!({"type": "message_start", "message": {"id": "msg_1"}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "EqQB"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "getColor", "input": {}}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"shade\":"}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": " \"dark\"}"}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
        json!({"type": "message_stop"}),
    ]);
    let (base, _) = mock(200, body).await;
    let out = run(&base, "k", req("x")).await;
    assert_eq!(out[0], ProviderEvent::ToolUse { id: "toolu_1".into(), name: "getColor".into(), input: json!({"shade": "dark"}) });
    let ProviderEvent::End { stop, assistant } = &out[1] else { panic!("{out:?}") };
    assert_eq!(*stop, Stop::ToolUse);
    assert_eq!(assistant[0], json!({"type": "thinking", "thinking": "", "signature": "EqQB"}));
}

#[tokio::test]
async fn http_errors_become_one_error_event() {
    for (status, kind, code) in [
        (429, "rate_limit_error", ProviderErrorCode::RateLimited),
        (401, "authentication_error", ProviderErrorCode::Unavailable),
        (529, "overloaded_error", ProviderErrorCode::Upstream),
    ] {
        let (base, _) = mock(status, json!({"type": "error", "error": {"type": kind, "message": "m sk-live"}}).to_string()).await;
        let out = run(&base, "sk-live", req("x")).await;
        assert_eq!(out.len(), 1, "{status}");
        let ProviderEvent::Error { code: got, message } = &out[0] else { panic!("{out:?}") };
        assert_eq!(*got, code);
        assert!(!message.contains("sk-live"));
    }
}

#[tokio::test]
async fn an_error_mid_stream_follows_the_text_already_sent() {
    let body = format!(
        "{}{}",
        sse(&[
            json!({"type": "message_start", "message": {"id": "m"}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "par"}}),
        ]),
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n"
    );
    let (base, _) = mock(200, body).await;
    let out = run(&base, "k", req("x")).await;
    assert_eq!(out, vec![
        ProviderEvent::Text("par".into()),
        ProviderEvent::Error { code: ProviderErrorCode::Upstream, message: "Overloaded".into() },
    ]);
}

#[tokio::test]
async fn refusal_and_length_stops_are_reported() {
    let (base, _) = mock(200, text_turn(&["I can"], "refusal")).await;
    assert!(matches!(run(&base, "k", req("x")).await.last(), Some(ProviderEvent::End { stop: Stop::Refusal, .. })));
    let (base, _) = mock(200, text_turn(&["cut"], "max_tokens")).await;
    assert!(matches!(run(&base, "k", req("x")).await.last(), Some(ProviderEvent::End { stop: Stop::MaxTokens, .. })));
}

#[tokio::test]
async fn a_stream_that_stops_early_or_an_unreachable_host_is_upstream_error() {
    let (base, _) = mock(200, sse(&[json!({"type": "message_start", "message": {"id": "m"}})])).await;
    assert!(matches!(run(&base, "k", req("x")).await.as_slice(), [ProviderEvent::Error { code: ProviderErrorCode::Upstream, .. }]));
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    assert!(matches!(run(&closed, "k", req("x")).await.as_slice(), [ProviderEvent::Error { code: ProviderErrorCode::Upstream, .. }]));
}
```

- [ ] **Step 13: Run them**

Run: `cargo test -p clax-server --test sample_anthropic`
Expected: PASS.

- [ ] **Step 14: Run everything and stage**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: PASS.

Stage the change (`git add Cargo.toml Cargo.lock crates/clax-core crates/clax-server crates/clax-cli`); do not commit. The controller commits it with the message:

```text
Read config.toml's [sample] table, and add the provider trait with a streaming Messages API provider and a deterministic stub
```


---

### Task 3: The sample routes: validation, the streamed call, tool rounds, cancellation, and the owner gate

Carried over from the earlier draft of this plan, renamed, with the owner's ruling applied (scan B4): only the owner's browser spends the key. The call and tool-result routes take `RequireToken` (401 `unauthorized` without the bearer token) and refuse a request without a viewer cookie with 403 `forbidden` (an agent or script holding the token is not a browser); the status route answers `available: false` to a caller without the token. Steps 13–15 add `GET /api/sample` and the `clax doctor` line (N9).

**Files:**
- Create: `crates/clax-server/src/sample/request.rs`, `crates/clax-server/src/sample/flight.rs`, `crates/clax-server/src/sample/json_reply.rs`
- Modify: `crates/clax-server/src/sample/mod.rs` (modules; pending tool calls on `Sampler`)
- Create: `crates/clax-server/src/routes/sample.rs`
- Modify: `crates/clax-server/src/routes/mod.rs` (`pub mod sample;` and the three routes)
- Modify: `crates/clax-cli/src/commands/doctor.rs` (`warn`, `sample_check`, the `sample` line)
- Test: unit tests in `request.rs`, `json_reply.rs`, `flight.rs`, `doctor.rs`; `crates/clax-server/tests/api_sample.rs` (new)

**Interfaces:**
- Consumes (Task 2): `Sampler::{provider, settings, reason, key_env}`, `OffReason`, `SampleSettings`, `ProviderRequest`, `Turn`, `Role`, `Block`, `ToolSpec`, `ProviderEvent`, `Stop`, `ProviderErrorCode::as_str`, `StubProvider::{new, requests}`, `SseParser`; phase 3 `SameOrigin`, `ViewerCookie`, `parse_id`; `crate::auth::{RequireToken, has_token}`.
- Produces:
  - `sample::request::{SampleBody, ToolBody, ImageBody, CacheBody, Verb, Tier, Prepared, CachePolicy, prepare, limits_json, FRAMING, JSON_FRAMING, MAX_PROMPT_BYTES = 65_536, MAX_TOOLS = 16, MAX_TOOL_DESCRIPTION_BYTES = 1024, MAX_TOOL_SCHEMA_BYTES = 4096, MAX_IMAGES = 5, MAX_IMAGE_BYTES = 5 * 1024 * 1024, MAX_INPUT_IMAGE_BYTES = 20_000_000, IMAGE_TYPES, MAX_TOOL_RESULT_BYTES = 32_768, DEFAULT_GC = 5 min, MAX_GC = 24 h}`; `prepare(body: SampleBody, settings: &SampleSettings, images: bool) -> Result<Prepared, ApiError>`; `Prepared { system, turns, tools, model, tier, verb, max_tokens, cache: CachePolicy, input_key: String }`; `CachePolicy::{Off, Window { gc: Duration, refresh: bool }}`.
  - `sample::json_reply::parse(text: &str) -> Option<Value>`.
  - `sample::flight::{Out, Done, Flight, drive}`: `Out::{Text(String), ToolCall { id, name, input }, Done(Done), Error { code: &'static str, message: String }}`, `Out::sse(&self) -> (&'static str, Value)`; `Done { text, truncated, model_tier_applied: Tier, value: Option<Value> }`; `Flight::new() -> Arc<Flight>`, `Flight::push(&self, Out)`, `Flight::set_abort(&self, AbortHandle)`, `Flight::is_finished(&self) -> bool`, `Flight::reader(self: &Arc<Self>) -> impl Stream<Item = Out> + Send + 'static`; `drive(sampler: Arc<Sampler>, call_id: String, p: Prepared, flight: Arc<Flight>, on_finish: impl FnOnce(Option<&Done>) + Send + 'static)`.
  - `Sampler::{open_call(&self, call_id: &str, viewer: Option<String>), expect_tool(&self, call_id: &str, tool_id: &str) -> Option<oneshot::Receiver<ToolOutput>>, deliver(&self, call_id: &str, viewer: Option<&str>, tool_id: &str, out: ToolOutput) -> Result<(), Deliver>, forget(&self, call_id: &str), open_calls(&self) -> usize}`; `ToolOutput { content: String, is_error: bool }`; `Deliver::{NotFound, Forbidden}`.
  - Routes `GET` and `POST /api/artifacts/{aid}/sample`, `POST /api/artifacts/{aid}/sample/{call}/tool_result` (`routes::sample::{status, sample, tool_result, SAMPLE_BODY_LIMIT}`), with the frames and refusals of the Shared contract (`calls_today` is `0` and `daily_call_cap` is the setting until Task 4 counts).

- [ ] **Step 1: Write the failing unit tests**

`crates/clax-server/src/sample/json_reply.rs`, tests only (add `pub mod flight; pub mod json_reply; pub mod request;` to `sample/mod.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::parse;
    use serde_json::json;

    #[test]
    fn reads_whole_fenced_or_framed_values() {
        assert_eq!(parse(" [1, 2] "), Some(json!([1, 2])));
        assert_eq!(parse("\"just a string\""), Some(json!("just a string")));
        assert_eq!(parse("```json\n{\"a\": 1}\n```"), Some(json!({"a": 1})));
        assert_eq!(parse("Here you go:\n{\"a\": 1}\nHope that helps."), Some(json!({"a": 1})));
    }

    #[test]
    fn refuses_two_values_and_no_value() {
        assert_eq!(parse("[1] and [2]"), None);
        assert_eq!(parse("no json here"), None);
        assert_eq!(parse("```\n{\"a\": 1}\n```\n```\n{\"b\": 2}\n```"), None);
        assert_eq!(parse(""), None);
    }
}
```

`crates/clax-server/src/sample/request.rs`, tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::SampleSettings;
    use crate::sample::provider::{Block, Role};
    use serde_json::json;

    fn body(v: serde_json::Value) -> SampleBody {
        serde_json::from_value(v).expect("a well-formed body")
    }

    fn code(r: Result<Prepared, ApiError>) -> &'static str {
        r.err().expect("refused").code
    }

    #[test]
    fn a_prompt_becomes_one_user_turn_with_the_framing_and_the_tier_model() {
        let s = SampleSettings::default();
        let p = prepare(body(json!({"input": "hi", "model_tier": "quick"})), &s, false).unwrap();
        assert_eq!(p.turns, vec![Turn { role: Role::User, content: vec![Block::Text("hi".into())] }]);
        assert_eq!(p.model, s.models.quick);
        assert_eq!(p.system, FRAMING);
        assert_eq!(p.cache, CachePolicy::Window { gc: DEFAULT_GC, refresh: false });
        let p = prepare(body(json!({"input": "hi", "verb": "json"})), &s, false).unwrap();
        assert_eq!(p.system, format!("{FRAMING}{JSON_FRAMING}"));
        assert_eq!(p.model, s.models.default);
    }

    #[test]
    fn turns_merge_same_role_neighbours_and_must_start_and_end_on_user() {
        let s = SampleSettings::default();
        let p = prepare(body(json!({"input": [
            {"role": "user", "content": "rules"}, {"role": "user", "content": "q1"},
            {"role": "assistant", "content": "a1"}, {"role": "user", "content": "q2"}]})), &s, false).unwrap();
        assert_eq!(p.turns.len(), 3);
        assert_eq!(p.turns[0].content, vec![Block::Text("rules\n\nq1".into())]);
        for bad in [
            json!([]),
            json!([{"role": "assistant", "content": "a"}, {"role": "user", "content": "q"}]),
            json!([{"role": "user", "content": "q"}, {"role": "assistant", "content": "a"}]),
            json!([{"role": "system", "content": "x"}]),
            json!([{"role": "user", "content": ""}]),
            json!([{"role": "user"}]),
            json!("   "),
            json!(42),
        ] {
            assert_eq!(code(prepare(body(json!({"input": bad})), &s, false)), "invalid_request", "{bad}");
        }
    }

    #[test]
    fn the_prompt_is_at_most_64_kib() {
        let s = SampleSettings::default();
        assert!(prepare(body(json!({"input": "x".repeat(MAX_PROMPT_BYTES)})), &s, false).is_ok());
        assert_eq!(code(prepare(body(json!({"input": "x".repeat(MAX_PROMPT_BYTES + 1)})), &s, false)), "prompt_too_large");
    }

    #[test]
    fn tools_are_checked_and_never_cached() {
        let s = SampleSettings::default();
        let tool = |name: &str| json!({"name": name, "description": "Returns the colour."});
        let p = prepare(body(json!({"input": "q", "tools": [tool("getColor")], "cache": false})), &s, false).unwrap();
        assert_eq!(p.tools[0].input_schema, json!({"type": "object", "properties": {}}));
        assert_eq!(p.cache, CachePolicy::Off);
        for bad in [
            json!({"input": "q", "tools": [tool("getColor")]}),
            json!({"input": "q", "tools": [tool("getColor")], "cache": {"gc_time_ms": 1000}}),
            json!({"input": "q", "tools": [tool("bad name")], "cache": false}),
            json!({"input": "q", "tools": [tool("a"), tool("a")], "cache": false}),
            json!({"input": "q", "tools": [{"name": "a", "description": ""}], "cache": false}),
            json!({"input": "q", "tools": [{"name": "a", "description": "d", "input_schema": {"type": "array"}}], "cache": false}),
            json!({"input": "q", "tools": (0..17).map(|i| tool(&format!("t{i}"))).collect::<Vec<_>>(), "cache": false}),
        ] {
            assert_eq!(code(prepare(body(bad.clone()), &s, false)), "invalid_request", "{bad}");
        }
    }

    #[test]
    fn cache_windows_are_positive_and_capped_at_a_day() {
        let s = SampleSettings::default();
        let p = prepare(body(json!({"input": "q", "cache": {"gc_time_ms": 1e12, "refresh": true}})), &s, false).unwrap();
        assert_eq!(p.cache, CachePolicy::Window { gc: MAX_GC, refresh: true });
        assert_eq!(code(prepare(body(json!({"input": "q", "cache": {"gc_time_ms": 0}})), &s, false)), "invalid_request");
        assert_eq!(code(prepare(body(json!({"input": "q", "cache": {"gc_time_ms": -5}})), &s, false)), "invalid_request");
    }

    #[test]
    fn images_need_a_provider_that_takes_them_and_ride_on_the_last_user_turn() {
        let s = SampleSettings::default();
        let img = json!({"media_type": "image/png", "data": "iVBORw0KGgo="});
        assert_eq!(code(prepare(body(json!({"input": "q", "images": [img]})), &s, false)), "images_unavailable");
        let p = prepare(body(json!({"input": [{"role": "user", "content": "a"}, {"role": "assistant", "content": "b"}, {"role": "user", "content": "c"}], "images": [img]})), &s, true).unwrap();
        assert!(matches!(&p.turns[2].content[..], [Block::Image { .. }, Block::Text(t)] if t == "c"));
        assert!(p.turns[0].content.iter().all(|b| matches!(b, Block::Text(_))));
        for bad in [
            json!({"media_type": "image/tiff", "data": "AAAA"}),
            json!({"media_type": "image/png", "data": "not base64!"}),
        ] {
            assert_eq!(code(prepare(body(json!({"input": "q", "images": [bad]})), &s, true)), "image_rejected");
        }
        let six: Vec<_> = (0..6).map(|_| img.clone()).collect();
        assert_eq!(code(prepare(body(json!({"input": "q", "images": six})), &s, true)), "image_rejected");
    }

    #[test]
    fn the_input_key_covers_verb_tier_input_and_images() {
        let s = SampleSettings::default();
        let k = |v: serde_json::Value| prepare(body(v), &s, true).unwrap().input_key;
        let base = k(json!({"input": "q"}));
        assert_eq!(base, k(json!({"input": "q", "cache": true})));
        assert_ne!(base, k(json!({"input": "q", "verb": "json"})));
        assert_ne!(base, k(json!({"input": "q", "model_tier": "quick"})));
        assert_ne!(base, k(json!({"input": "Q"})));
        assert_ne!(base, k(json!({"input": "q", "images": [{"media_type": "image/png", "data": "iVBORw0KGgo="}]})));
    }

    #[test]
    fn limits_report_images_only_when_the_provider_takes_them() {
        assert_eq!(limits_json(false), json!({"maxPromptBytes": 65536, "tools": {"maxCount": 16}}));
        assert_eq!(limits_json(true)["images"], json!({"maxCount": 5, "maxInputBytes": 20000000, "mediaTypes": ["image/jpeg", "image/png", "image/webp", "image/gif"]}));
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p clax-server --lib sample::`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `json_reply` and `request`**

`crates/clax-server/src/sample/json_reply.rs`, above its tests:

```rust
//! Reading a reply as one JSON value for `sample.json` (`sample.d.ts`): the
//! whole reply; else the body of its one Markdown code fence; else the text
//! from the first `{` or `[` to the last `}` or `]`.

use serde_json::Value;

fn one_fence(t: &str) -> Option<&str> {
    let open = t.find("```")?;
    let after = &t[open + 3..];
    let body = &after[after.find('\n')? + 1..];
    let close = body.find("```")?;
    if body[close + 3..].contains("```") {
        return None;
    }
    Some(&body[..close])
}

/// The JSON value the reply holds, or `None`.
pub fn parse(text: &str) -> Option<Value> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str(t) {
        return Some(v);
    }
    if let Some(body) = one_fence(t)
        && let Ok(v) = serde_json::from_str(body.trim())
    {
        return Some(v);
    }
    let start = t.find(['{', '['])?;
    let end = t.rfind(['}', ']'])?;
    if end < start {
        return None;
    }
    serde_json::from_str(&t[start..=end]).ok()
}
```

`crates/clax-server/src/sample/request.rs`, above its tests:

```rust
//! The body of `POST /api/artifacts/<aid>/sample` and its checks
//! (`sample.d.ts`): what a page may ask, turned into provider turns under the
//! fixed framing. A refusal here is a 400 before any stream starts, and nothing
//! reaches the provider.

use super::SampleSettings;
use super::provider::{Block, Role, ToolSpec, Turn};
use crate::error::ApiError;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

pub const MAX_PROMPT_BYTES: usize = 65_536;
pub const MAX_TOOLS: usize = 16;
pub const MAX_TOOL_DESCRIPTION_BYTES: usize = 1024;
pub const MAX_TOOL_SCHEMA_BYTES: usize = 4096;
pub const MAX_TOOL_RESULT_BYTES: usize = 32_768;
pub const MAX_IMAGES: usize = 5;
/// Largest image the daemon accepts after the bridge downsized it.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// Largest file a page may hand the bridge (`limits().images.maxInputBytes`).
pub const MAX_INPUT_IMAGE_BYTES: u64 = 20_000_000;
pub const IMAGE_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/webp", "image/gif"];
pub const DEFAULT_GC: Duration = Duration::from_secs(300);
pub const MAX_GC: Duration = Duration::from_secs(86_400);

/// The fixed framing every call is sent under; pages never set a system prompt.
pub const FRAMING: &str = "You are answering a request that a web page made through its sample() call. The page's code wrote every user turn, and any assistant turns are earlier replies the page is replaying as context; a person viewing the page triggered the request. You cannot browse the web, you remember nothing between requests, and you have no tools except those the page lists. Answer the request directly.";
/// Added to [`FRAMING`] for `sample.json`.
pub const JSON_FRAMING: &str = " A program will parse your final message as JSON: reply with exactly one JSON value and nothing else.";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Verb {
    #[default]
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Quick,
    #[default]
    Default,
    Complex,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolBody {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: Option<Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageBody {
    pub media_type: String,
    pub data: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum CacheBody {
    Flag(bool),
    Window {
        #[serde(default)]
        gc_time_ms: Option<f64>,
        #[serde(default)]
        refresh: bool,
    },
}

impl Default for CacheBody {
    fn default() -> Self {
        CacheBody::Flag(true)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleBody {
    pub input: Value,
    #[serde(default)]
    pub verb: Verb,
    #[serde(default)]
    pub model_tier: Tier,
    #[serde(default)]
    pub tools: Vec<ToolBody>,
    #[serde(default)]
    pub images: Vec<ImageBody>,
    #[serde(default)]
    pub cache: CacheBody,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachePolicy {
    Off,
    Window { gc: Duration, refresh: bool },
}

/// A checked call, ready for [`super::flight::drive`].
#[derive(Clone, Debug)]
pub struct Prepared {
    pub system: String,
    pub turns: Vec<Turn>,
    pub tools: Vec<ToolSpec>,
    pub model: String,
    pub tier: Tier,
    pub verb: Verb,
    pub max_tokens: u32,
    pub cache: CachePolicy,
    /// Identifies the question (verb, tier, every turn, the images) for the answer cache.
    pub input_key: String,
}

fn invalid(m: impl Into<String>) -> ApiError {
    ApiError::bad_request("invalid_request", m)
}

fn turns_of(input: &Value) -> Result<Vec<(Role, String)>, ApiError> {
    match input {
        Value::String(s) if !s.trim().is_empty() => Ok(vec![(Role::User, s.clone())]),
        Value::String(_) => Err(invalid("input is empty")),
        Value::Array(items) => {
            if items.is_empty() {
                return Err(invalid("input has no turns"));
            }
            let mut out = Vec::with_capacity(items.len());
            for (i, t) in items.iter().enumerate() {
                let role = match t.get("role").and_then(Value::as_str) {
                    Some("user") => Role::User,
                    Some("assistant") => Role::Assistant,
                    _ => return Err(invalid(format!("input[{i}].role must be \"user\" or \"assistant\""))),
                };
                let content = t.get("content").and_then(Value::as_str).unwrap_or("");
                if content.is_empty() {
                    return Err(invalid(format!("input[{i}].content must be a non-empty string")));
                }
                out.push((role, content.to_string()));
            }
            if out[0].0 != Role::User || out[out.len() - 1].0 != Role::User {
                return Err(invalid("input turns must start and end with a user turn"));
            }
            Ok(out)
        }
        _ => Err(invalid("input is a prompt string or an array of {role, content} turns")),
    }
}

fn tool_ok(name: &str) -> bool {
    (1..=128).contains(&name.len()) && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// Checks `body` and builds the provider turns. `images` is whether the provider takes images.
///
/// # Errors
/// 400 with `invalid_request`, `prompt_too_large`, `images_unavailable`, or `image_rejected`.
pub fn prepare(body: SampleBody, settings: &SampleSettings, images: bool) -> Result<Prepared, ApiError> {
    let pairs = turns_of(&body.input)?;
    let bytes: usize = pairs.iter().map(|(_, c)| c.len()).sum();
    if bytes > MAX_PROMPT_BYTES {
        return Err(ApiError::bad_request("prompt_too_large", format!("the input is {bytes} bytes; the limit is {MAX_PROMPT_BYTES}")));
    }
    if body.tools.len() > MAX_TOOLS {
        return Err(invalid(format!("at most {MAX_TOOLS} tools per call")));
    }
    let mut tools = Vec::with_capacity(body.tools.len());
    for (i, t) in body.tools.iter().enumerate() {
        if !tool_ok(&t.name) {
            return Err(invalid(format!("tools[{i}].name must be 1-128 of A-Z a-z 0-9 _ -")));
        }
        if tools.iter().any(|x: &ToolSpec| x.name == t.name) {
            return Err(invalid(format!("tools[{i}].name '{}' is used twice", t.name)));
        }
        if t.description.trim().is_empty() || t.description.len() > MAX_TOOL_DESCRIPTION_BYTES {
            return Err(invalid(format!("tools[{i}].description must be 1-{MAX_TOOL_DESCRIPTION_BYTES} bytes")));
        }
        let schema = t.input_schema.clone().unwrap_or_else(|| json!({"type": "object", "properties": {}}));
        if schema.get("type").and_then(Value::as_str) != Some("object") || schema.to_string().len() > MAX_TOOL_SCHEMA_BYTES {
            return Err(invalid(format!("tools[{i}].inputSchema must be a JSON Schema object of type \"object\", at most {MAX_TOOL_SCHEMA_BYTES} bytes")));
        }
        tools.push(ToolSpec { name: t.name.clone(), description: t.description.clone(), input_schema: schema });
    }
    let cache = match body.cache {
        CacheBody::Flag(false) => CachePolicy::Off,
        CacheBody::Flag(true) => CachePolicy::Window { gc: DEFAULT_GC, refresh: false },
        CacheBody::Window { gc_time_ms, refresh } => {
            let gc = match gc_time_ms {
                None => DEFAULT_GC,
                Some(ms) if ms.is_finite() && ms > 0.0 => Duration::from_secs_f64(ms / 1000.0).min(MAX_GC),
                Some(_) => return Err(invalid("cache.gcTime must be a number of milliseconds greater than zero")),
            };
            CachePolicy::Window { gc, refresh }
        }
    };
    if !tools.is_empty() && cache != CachePolicy::Off {
        return Err(invalid("a call with tools is never cached: omit cache or pass false"));
    }
    if !body.images.is_empty() && !images {
        return Err(ApiError::bad_request("images_unavailable", "this view cannot send images"));
    }
    if body.images.len() > MAX_IMAGES {
        return Err(ApiError::bad_request("image_rejected", format!("at most {MAX_IMAGES} images per call")));
    }
    let mut image_blocks = Vec::with_capacity(body.images.len());
    let mut image_hash = std::hash::DefaultHasher::new();
    for (i, img) in body.images.iter().enumerate() {
        if !IMAGE_TYPES.contains(&img.media_type.as_str()) {
            return Err(ApiError::bad_request("image_rejected", format!("images[{i}] is {}; images are JPEG, PNG, WebP, or GIF", img.media_type)));
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&img.data)
            .map_err(|_| ApiError::bad_request("image_rejected", format!("images[{i}] is not base64")))?;
        if decoded.len() > MAX_IMAGE_BYTES {
            return Err(ApiError::bad_request("image_rejected", format!("images[{i}] is over {MAX_IMAGE_BYTES} bytes")));
        }
        std::hash::Hash::hash(&(img.media_type.as_str(), decoded.as_slice()), &mut image_hash);
        image_blocks.push(Block::Image { media_type: img.media_type.clone(), data: img.data.clone() });
    }
    let mut turns: Vec<Turn> = Vec::new();
    for (role, content) in pairs {
        match turns.last_mut() {
            Some(t) if t.role == role => {
                if let Some(Block::Text(prev)) = t.content.last_mut() {
                    prev.push_str("\n\n");
                    prev.push_str(&content);
                }
            }
            _ => turns.push(Turn { role, content: vec![Block::Text(content)] }),
        }
    }
    if let Some(last) = turns.last_mut() {
        last.content.splice(0..0, image_blocks);
    }
    let system = match body.verb {
        Verb::Text => FRAMING.to_string(),
        Verb::Json => format!("{FRAMING}{JSON_FRAMING}"),
    };
    let model = match body.model_tier {
        Tier::Quick => settings.models.quick.clone(),
        Tier::Default => settings.models.default.clone(),
        Tier::Complex => settings.models.complex.clone(),
    };
    let input_key = format!(
        "{}|{}|{}|{:016x}",
        serde_json::to_string(&body.verb).expect("verb serialises"),
        serde_json::to_string(&body.model_tier).expect("tier serialises"),
        body.input,
        std::hash::Hasher::finish(&image_hash)
    );
    Ok(Prepared { system, turns, tools, model, tier: body.model_tier, verb: body.verb, max_tokens: settings.max_tokens, cache, input_key })
}

/// `sample.d.ts`'s `SampleLimits`, with `images` only when the provider takes them.
pub fn limits_json(images: bool) -> Value {
    let mut v = json!({"maxPromptBytes": MAX_PROMPT_BYTES, "tools": {"maxCount": MAX_TOOLS}});
    if images {
        v["images"] = json!({"maxCount": MAX_IMAGES, "maxInputBytes": MAX_INPUT_IMAGE_BYTES, "mediaTypes": IMAGE_TYPES});
    }
    v
}
```

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -p clax-server --lib sample::request sample::json_reply`
Expected: PASS.

- [ ] **Step 5: Write the failing flight tests**

`crates/clax-server/src/sample/flight.rs`, tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn readers_replay_from_the_start_and_follow_until_the_end() {
        let f = Flight::new();
        f.push(Out::Text("a".into()));
        let early = f.reader();
        f.push(Out::Text("b".into()));
        let late = f.reader();
        let pusher = f.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            pusher.push(Out::Done(Done { text: "ab".into(), truncated: false, model_tier_applied: Tier::Default, value: None }));
        });
        let (x, y): (Vec<Out>, Vec<Out>) = tokio::join!(early.collect(), late.collect());
        assert_eq!(x, y);
        assert_eq!(x.len(), 3);
        assert!(f.is_finished());
    }

    #[tokio::test]
    async fn the_last_reader_leaving_aborts_an_unfinished_flight() {
        let f = Flight::new();
        let task = tokio::spawn(std::future::pending::<()>());
        f.set_abort(task.abort_handle());
        let (a, b) = (f.reader(), f.reader());
        drop(a);
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        drop(b);
        let r = task.await;
        assert!(r.unwrap_err().is_cancelled());
    }

    #[test]
    fn frames_serialise_as_the_sse_contract() {
        assert_eq!(Out::Text("hi".into()).sse(), ("text", serde_json::json!({"delta": "hi"})));
        let d = Done { text: "t".into(), truncated: true, model_tier_applied: Tier::Quick, value: None };
        assert_eq!(Out::Done(d).sse(), ("done", serde_json::json!({"text": "t", "truncated": true, "model_tier_applied": "quick"})));
        assert_eq!(
            Out::Error { code: "refused", message: "no".into() }.sse(),
            ("error", serde_json::json!({"code": "refused", "message": "no"}))
        );
    }
}
```

- [ ] **Step 6: Run them to verify they fail**

Run: `cargo test -p clax-server --lib sample::flight`
Expected: FAIL to compile.

- [ ] **Step 7: Implement pending tool calls, `Flight`, and `drive`**

In `crates/clax-server/src/sample/mod.rs`, add the pending-call registry (a field `pending: Mutex<HashMap<String, PendingCall>>` on `Sampler`, initialised `Default::default()` in `Sampler::build`):

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// A page tool's answer to one tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

/// Why a tool result was not delivered.
#[derive(Debug, PartialEq)]
pub enum Deliver {
    /// No such running call, or it is not waiting for that tool call.
    NotFound,
    /// The call belongs to another viewer.
    Forbidden,
}

#[derive(Default)]
struct PendingCall {
    /// The viewer cookie that started the call (memory only; never sent anywhere).
    viewer: Option<String>,
    tools: HashMap<String, oneshot::Sender<ToolOutput>>,
}

impl Sampler {
    /// Registers a running call so its tool results can be delivered.
    pub fn open_call(&self, call_id: &str, viewer: Option<String>) {
        self.pending.lock().expect("pending lock").insert(call_id.to_string(), PendingCall { viewer, tools: HashMap::new() });
    }

    /// Waits for the result of tool call `tool_id` of `call_id`; `None` when the call is gone.
    pub fn expect_tool(&self, call_id: &str, tool_id: &str) -> Option<oneshot::Receiver<ToolOutput>> {
        let mut pending = self.pending.lock().expect("pending lock");
        let call = pending.get_mut(call_id)?;
        let (tx, rx) = oneshot::channel();
        call.tools.insert(tool_id.to_string(), tx);
        Some(rx)
    }

    /// Hands `out` to the round waiting on `tool_id`, if `viewer` started the call.
    pub fn deliver(&self, call_id: &str, viewer: Option<&str>, tool_id: &str, out: ToolOutput) -> Result<(), Deliver> {
        let mut pending = self.pending.lock().expect("pending lock");
        let call = pending.get_mut(call_id).ok_or(Deliver::NotFound)?;
        if call.viewer.as_deref() != viewer {
            return Err(Deliver::Forbidden);
        }
        let tx = call.tools.remove(tool_id).ok_or(Deliver::NotFound)?;
        tx.send(out).map_err(|_| Deliver::NotFound)
    }

    /// Drops a call and every tool wait it had.
    pub fn forget(&self, call_id: &str) {
        self.pending.lock().expect("pending lock").remove(call_id);
    }

    /// Calls still registered (tests).
    pub fn open_calls(&self) -> usize {
        self.pending.lock().expect("pending lock").len()
    }
}
```

`crates/clax-server/src/sample/flight.rs`, above its tests:

```rust
//! One `sample()` call's run and its frames. [`drive`] asks the provider
//! round by round, relays page tool calls and waits for their results, and
//! pushes [`Out`] frames into a [`Flight`]; each reader replays a flight's
//! frames from the start and then follows it live. When the last reader goes
//! away before the flight ends, the run is aborted: the provider request is
//! dropped (the key stops paying) and the call's tool waits are forgotten.

use super::provider::{Block, ProviderEvent, ProviderRequest, Role, Stop, Turn};
use super::request::{Prepared, Tier, Verb};
use super::{Sampler, ToolOutput, json_reply};
use futures::{Stream, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio::task::AbortHandle;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Done {
    pub text: String,
    pub truncated: bool,
    pub model_tier_applied: Tier,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// One frame of a call's stream (the SSE frames of `docs/contract.md` "Sample protocol").
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    Text(String),
    ToolCall { id: String, name: String, input: Value },
    Done(Done),
    Error { code: &'static str, message: String },
}

impl Out {
    /// The SSE event name and data.
    pub fn sse(&self) -> (&'static str, Value) {
        match self {
            Out::Text(d) => ("text", json!({"delta": d})),
            Out::ToolCall { id, name, input } => ("tool_call", json!({"id": id, "name": name, "input": input})),
            Out::Done(d) => ("done", serde_json::to_value(d).expect("done serialises")),
            Out::Error { code, message } => ("error", json!({"code": code, "message": message})),
        }
    }

    fn ends(&self) -> bool {
        matches!(self, Out::Done(_) | Out::Error { .. })
    }
}

#[derive(Default)]
pub struct Flight {
    frames: Mutex<Vec<Out>>,
    finished: AtomicBool,
    notify: Notify,
    readers: AtomicUsize,
    abort: Mutex<Option<AbortHandle>>,
}

impl Flight {
    pub fn new() -> Arc<Flight> {
        Arc::default()
    }

    /// Appends a frame; a `Done` or `Error` ends the flight (later pushes are ignored).
    pub fn push(&self, o: Out) {
        if self.is_finished() {
            return;
        }
        let ends = o.ends();
        self.frames.lock().expect("frames lock").push(o);
        if ends {
            self.finished.store(true, Ordering::SeqCst);
        }
        self.notify.notify_waiters();
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// The run to abort when the last reader leaves an unfinished flight.
    pub fn set_abort(&self, h: AbortHandle) {
        *self.abort.lock().expect("abort lock") = Some(h);
    }

    /// Every frame so far, then each new one, until the flight ends.
    pub fn reader(self: &Arc<Self>) -> impl Stream<Item = Out> + Send + 'static {
        self.readers.fetch_add(1, Ordering::SeqCst);
        let guard = ReaderGuard(self.clone());
        futures::stream::unfold((guard, 0usize), |(guard, next)| async move {
            loop {
                let flight = guard.0.clone();
                let notified = flight.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if let Some(o) = flight.frames.lock().expect("frames lock").get(next).cloned() {
                    return Some((o, (guard, next + 1)));
                }
                if flight.is_finished() {
                    return None;
                }
                notified.await;
            }
        })
    }
}

struct ReaderGuard(Arc<Flight>);

impl Drop for ReaderGuard {
    fn drop(&mut self) {
        if self.0.readers.fetch_sub(1, Ordering::SeqCst) == 1
            && !self.0.is_finished()
            && let Some(h) = self.0.abort.lock().expect("abort lock").take()
        {
            h.abort();
        }
    }
}

/// Calls `f(None)` if dropped before [`Finish::done`]: the run was aborted or failed.
struct Finish<F: FnOnce(Option<&Done>)>(Option<F>);

impl<F: FnOnce(Option<&Done>)> Finish<F> {
    fn done(&mut self, d: Option<&Done>) {
        if let Some(f) = self.0.take() {
            f(d);
        }
    }
}

impl<F: FnOnce(Option<&Done>)> Drop for Finish<F> {
    fn drop(&mut self) {
        self.done(None);
    }
}

struct Forget(Arc<Sampler>, String);

impl Drop for Forget {
    fn drop(&mut self) {
        self.0.forget(&self.1);
    }
}

fn fail(flight: &Flight, code: &'static str, message: impl Into<String>) {
    flight.push(Out::Error { code, message: message.into() });
}

/// Runs call `call_id` (already registered with [`Sampler::open_call`]) and
/// pushes its frames into `flight`. `on_finish` runs once: with the answer
/// just before a successful `Done` is pushed, with `None` on any failure or
/// when the run is aborted.
pub async fn drive(sampler: Arc<Sampler>, call_id: String, p: Prepared, flight: Arc<Flight>, on_finish: impl FnOnce(Option<&Done>) + Send + 'static) {
    let _forget = Forget(sampler.clone(), call_id.clone());
    let mut finish = Finish(Some(on_finish));
    let Some(provider) = sampler.provider().cloned() else {
        fail(&flight, "sampling_disabled", "no sample provider is configured on this machine");
        return;
    };
    let rounds = if p.tools.is_empty() { 1 } else { sampler.settings.max_rounds.max(1) };
    let mut turns = p.turns.clone();
    let mut text = String::new();
    let mut last_round_text = String::new();
    let mut truncated = false;
    for round in 0..rounds {
        let req = ProviderRequest {
            model: p.model.clone(),
            max_tokens: p.max_tokens,
            system: p.system.clone(),
            messages: turns.clone(),
            tools: p.tools.clone(),
            final_round: round + 1 == rounds,
        };
        let mut stream = provider.stream(req);
        let mut calls = Vec::new();
        let mut end = None;
        last_round_text.clear();
        while let Some(ev) = stream.next().await {
            match ev {
                ProviderEvent::Text(d) if d.is_empty() => {}
                ProviderEvent::Text(d) => {
                    if last_round_text.is_empty() && !text.is_empty() {
                        text.push_str("\n\n");
                        flight.push(Out::Text("\n\n".into()));
                    }
                    text.push_str(&d);
                    last_round_text.push_str(&d);
                    flight.push(Out::Text(d));
                }
                ProviderEvent::ToolUse { id, name, input } => calls.push((id, name, input)),
                ProviderEvent::End { stop, assistant } => {
                    end = Some((stop, assistant));
                    break;
                }
                ProviderEvent::Error { code, message } => {
                    fail(&flight, code.as_str(), message);
                    return;
                }
            }
        }
        let Some((stop, assistant)) = end else {
            fail(&flight, "upstream_error", "the provider's answer ended early");
            return;
        };
        match stop {
            Stop::Refusal => {
                fail(&flight, "refused", "Claude declined this request");
                return;
            }
            Stop::MaxTokens => {
                truncated = true;
                break;
            }
            Stop::ToolUse if !calls.is_empty() && round + 1 < rounds => {
                turns.push(Turn { role: Role::Assistant, content: assistant.into_iter().map(Block::Raw).collect() });
                let mut waits = Vec::with_capacity(calls.len());
                for (id, ..) in &calls {
                    let Some(rx) = sampler.expect_tool(&call_id, id) else {
                        fail(&flight, "upstream_error", "the call was closed while it waited for a tool");
                        return;
                    };
                    waits.push(rx);
                }
                for (id, name, input) in &calls {
                    flight.push(Out::ToolCall { id: id.clone(), name: name.clone(), input: input.clone() });
                }
                let limit = sampler.settings.tool_timeout;
                let outputs = futures::future::join_all(waits.into_iter().map(|rx| async move {
                    match tokio::time::timeout(limit, rx).await {
                        Ok(Ok(o)) => o,
                        _ => ToolOutput { content: format!("Error: the tool did not answer within {} s", limit.as_secs()), is_error: true },
                    }
                }))
                .await;
                let results = calls
                    .iter()
                    .zip(outputs)
                    .map(|((id, ..), o)| Block::ToolResult { tool_use_id: id.clone(), content: o.content, is_error: o.is_error })
                    .collect();
                turns.push(Turn { role: Role::User, content: results });
            }
            Stop::ToolUse | Stop::EndTurn => break,
        }
    }
    if text.trim().is_empty() {
        fail(&flight, "empty_completion", "Claude produced no text");
        return;
    }
    let value = match p.verb {
        Verb::Text => None,
        Verb::Json if truncated => {
            fail(&flight, "invalid_json", "the answer was cut short by the length limit before its JSON was complete");
            return;
        }
        Verb::Json => match json_reply::parse(&last_round_text) {
            Some(v) => Some(v),
            None => {
                fail(&flight, "invalid_json", "the final message holds no parseable JSON value");
                return;
            }
        },
    };
    let done = Done { text, truncated, model_tier_applied: p.tier, value };
    // Forget the call before anyone can read `done`, so its tool waits are gone by then.
    sampler.forget(&call_id);
    finish.done(Some(&done));
    flight.push(Out::Done(done));
}
```

- [ ] **Step 8: Run the flight tests to verify they pass**

Run: `cargo test -p clax-server --lib sample::flight`
Expected: PASS.

- [ ] **Step 9: Write the failing route tests**

`crates/clax-server/tests/api_sample.rs`:

```rust
mod common;
use clax_server::sample::sse::SseParser;
use clax_server::sample::stub::StubProvider;
use clax_server::sample::{SampleSettings, Sampler};
use clax_server::sample::provider::{Block, Role};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// A test daemon whose sample provider is the stub (5 ms per piece); returns the stub and the sampler.
async fn stub_server(images: bool, settings: SampleSettings) -> (TestServer, StubProvider, Arc<Sampler>) {
    stub_server_with(images, settings, Duration::from_millis(5)).await
}

/// [`stub_server`] with the stub pausing `delay` before each streamed piece.
async fn stub_server_with(images: bool, settings: SampleSettings, delay: Duration) -> (TestServer, StubProvider, Arc<Sampler>) {
    let stub = StubProvider::new(images, delay);
    let sampler = Arc::new(Sampler::new(Arc::new(stub.clone()), settings));
    let s = sampler.clone();
    let ts = TestServer::spawn_with(move |st| st.sample = s).await;
    (ts, stub, sampler)
}

async fn artifact(ts: &TestServer, caps: Value) -> String {
    let res = ts
        .post_json("/api/artifacts", json!({"title": "Ask", "capabilities": caps, "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}}))
        .await;
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["artifact"]["id"].as_str().unwrap().to_string()
}

/// One sample call's open stream.
struct Call {
    body: std::pin::Pin<Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>,
    parser: SseParser,
    queued: std::collections::VecDeque<(String, Value)>,
}

impl Call {
    /// The next frame within 10 s, keep-alives skipped.
    async fn next(&mut self) -> (String, Value) {
        use futures::StreamExt;
        loop {
            if let Some(f) = self.queued.pop_front() {
                return f;
            }
            let chunk = tokio::time::timeout(Duration::from_secs(10), self.body.next()).await.expect("a frame within 10 s").expect("open").unwrap();
            for e in self.parser.push(&chunk) {
                self.queued.push_back((e.event, serde_json::from_str(&e.data).unwrap()));
            }
        }
    }

    /// Frames up to and including `done` or `error`.
    async fn rest(&mut self) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        loop {
            let f = self.next().await;
            let end = f.0 == "done" || f.0 == "error";
            out.push(f);
            if end {
                return out;
            }
        }
    }
}

fn text_of(frames: &[(String, Value)]) -> String {
    frames.iter().filter(|(n, _)| n == "text").map(|(_, d)| d["delta"].as_str().unwrap()).collect()
}

/// The viewer cookie of the owner's browser in these tests when a test names none.
const OWNER_BROWSER: &str = "owner-browser";

/// Starts a call as the owner's browser: the bearer token plus a viewer
/// cookie (`cookie`, else [`OWNER_BROWSER`]).
async fn start(ts: &TestServer, aid: &str, body: Value, cookie: Option<&str>) -> Result<Call, (u16, Value)> {
    let c = cookie.unwrap_or(OWNER_BROWSER);
    let r = ts
        .authed(ts.client.post(format!("{}/api/artifacts/{aid}/sample", ts.base)))
        .header("cookie", format!("clax_viewer={c}"))
        .json(&body);
    let res = r.send().await.unwrap();
    if !res.status().is_success() {
        let s = res.status().as_u16();
        return Err((s, res.json().await.unwrap_or(Value::Null)));
    }
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    Ok(Call { body: Box::pin(res.bytes_stream()), parser: SseParser::default(), queued: Default::default() })
}

#[tokio::test]
async fn streams_text_then_done_under_the_framing_and_the_tier_model() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let mut c = start(&ts, &aid, json!({"input": "hello", "model_tier": "complex"}), None).await.unwrap();
    let (name, s) = c.next().await;
    assert_eq!(name, "start");
    assert_eq!(s["cached"], false);
    assert!(s["call_id"].as_str().is_some_and(|id| id.len() == 26));
    let frames = c.rest().await;
    assert_eq!(text_of(&frames), "echo: hello");
    assert_eq!(frames.last().unwrap(), &("done".to_string(), json!({"text": "echo: hello", "truncated": false, "model_tier_applied": "complex"})));
    let req = &stub.requests()[0];
    assert_eq!(req.model, SampleSettings::default().models.complex);
    assert!(req.system.starts_with("You are answering a request that a web page made"));
}

#[tokio::test]
async fn a_tool_round_trip_joins_the_rounds_text_with_a_blank_line() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let viewer = ts.viewer(None).await;
    let tools = json!([{"name": "getColor", "description": "Returns the page's accent colour."}]);
    let mut c = start(&ts, &aid, json!({"input": "[[tool:getColor]]", "tools": tools, "cache": false}), Some(&viewer.cookie)).await.unwrap();
    let call_id = c.next().await.1["call_id"].as_str().unwrap().to_string();
    assert_eq!(c.next().await, ("text".into(), json!({"delta": "Checking."})));
    assert_eq!(c.next().await, ("tool_call".into(), json!({"id": "toolu_stub_1", "name": "getColor", "input": {}})));
    let url = format!("{}/api/artifacts/{aid}/sample/{call_id}/tool_result", ts.base);
    let other = ts.viewer(None).await;
    let res = ts.authed(ts.client.post(&url)).header("cookie", format!("clax_viewer={}", other.cookie)).json(&json!({"id": "toolu_stub_1", "content": "red"})).send().await.unwrap();
    assert_eq!(res.status(), 403);
    let res = ts.authed(ts.client.post(&url)).header("cookie", format!("clax_viewer={}", viewer.cookie)).json(&json!({"id": "toolu_nope", "content": "x"})).send().await.unwrap();
    assert_eq!(res.status(), 404);
    let res = ts.authed(ts.client.post(&url)).header("cookie", format!("clax_viewer={}", viewer.cookie)).json(&json!({"id": "toolu_stub_1", "content": "teal"})).send().await.unwrap();
    assert_eq!(res.status(), 204);
    let frames = c.rest().await;
    assert_eq!(text_of(&frames), "\n\ntool said: teal");
    assert_eq!(frames.last().unwrap().1["text"], "Checking.\n\ntool said: teal");
    let second = &stub.requests()[1];
    assert_eq!(second.messages[1].role, Role::Assistant);
    assert!(matches!(&second.messages[1].content[..], [Block::Raw(_), Block::Raw(_)]));
    assert!(matches!(&second.messages[2].content[..], [Block::ToolResult { content, is_error: false, .. }] if content == "teal"));
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/sample/01J00000000000000000000000/tool_result", ts.base))).header("cookie", format!("clax_viewer={}", viewer.cookie)).json(&json!({"id": "x", "content": "y"})).send().await.unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn a_tool_that_never_answers_times_out_into_an_error_result() {
    let settings = SampleSettings { tool_timeout: Duration::from_millis(200), ..SampleSettings::default() };
    let (ts, stub, sampler) = stub_server(false, settings).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let tools = json!([{"name": "stuck", "description": "Never answers."}]);
    let mut c = start(&ts, &aid, json!({"input": "[[tool:stuck]]", "tools": tools, "cache": false}), None).await.unwrap();
    let frames = c.rest().await;
    assert_eq!(frames.last().unwrap().0, "done");
    assert!(text_of(&frames).contains("tool said: Error: the tool did not answer within 0 s"));
    assert!(matches!(&stub.requests()[1].messages[2].content[..], [Block::ToolResult { is_error: true, .. }]));
    assert_eq!(sampler.open_calls(), 0);
}

#[tokio::test]
async fn dropping_the_stream_cancels_the_call() {
    let (ts, stub, sampler) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let tools = json!([{"name": "stuck", "description": "Never answers."}]);
    let mut waiting = start(&ts, &aid, json!({"input": "[[tool:stuck]]", "tools": tools, "cache": false}), None).await.unwrap();
    while waiting.next().await.0 != "tool_call" {}
    let mut slow = start(&ts, &aid, json!({"input": "[[slow]]", "cache": false}), None).await.unwrap();
    while slow.next().await.0 != "text" {}
    assert_eq!(sampler.open_calls(), 2);
    drop(waiting);
    drop(slow);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while sampler.open_calls() > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(sampler.open_calls(), 0);
    assert_eq!(stub.requests().len(), 2, "no further round was asked for");
}

#[tokio::test]
async fn provider_outcomes_map_to_the_contract() {
    let (ts, _, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    for (prompt, code) in [
        ("[[error:rate_limited]]", "rate_limited"),
        ("[[error:unavailable]]", "sampling_disabled"),
        ("[[refuse]]", "refused"),
        ("[[empty]]", "empty_completion"),
    ] {
        let frames = start(&ts, &aid, json!({"input": prompt, "cache": false}), None).await.unwrap().rest().await;
        assert_eq!(frames.last().unwrap().1["code"], code, "{prompt}");
    }
    let frames = start(&ts, &aid, json!({"input": "[[error-after:upstream_error]]", "cache": false}), None).await.unwrap().rest().await;
    assert_eq!(text_of(&frames), "partial ");
    assert_eq!(frames.last().unwrap().1["code"], "upstream_error");
    let frames = start(&ts, &aid, json!({"input": "[[truncate]]", "cache": false}), None).await.unwrap().rest().await;
    assert_eq!(frames.last().unwrap().1, json!({"text": "cut", "truncated": true, "model_tier_applied": "default"}));
}

#[tokio::test]
async fn json_calls_resolve_a_value_or_invalid_json() {
    let (ts, _, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let frames = start(&ts, &aid, json!({"input": "[[say:Here: {\"a\": 1}]]", "verb": "json"}), None).await.unwrap().rest().await;
    assert_eq!(frames.last().unwrap().1["value"], json!({"a": 1}));
    let frames = start(&ts, &aid, json!({"input": "[[say:no json at all]]", "verb": "json"}), None).await.unwrap().rest().await;
    assert_eq!(frames.last().unwrap().1["code"], "invalid_json");
    let frames = start(&ts, &aid, json!({"input": "[[truncate]]", "verb": "json"}), None).await.unwrap().rest().await;
    assert_eq!(frames.last().unwrap().1["code"], "invalid_json");
}

#[tokio::test]
async fn bad_calls_are_refused_before_any_stream() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    for (body, code) in [
        (json!({"input": ""}), "invalid_request"),
        (json!({"input": [{"role": "user", "content": "q"}, {"role": "assistant", "content": "a"}]}), "invalid_request"),
        (json!({"input": "x".repeat(65_537)}), "prompt_too_large"),
        (json!({"input": "q", "images": [{"media_type": "image/png", "data": "iVBORw0KGgo="}]}), "images_unavailable"),
        (json!({"input": "q", "tools": [{"name": "a", "description": "d"}]}), "invalid_request"),
        (json!({"input": "q", "model_tier": "huge"}), "invalid_request"),
        (json!({"prompt": "q"}), "invalid_request"),
    ] {
        let (status, err) = start(&ts, &aid, body.clone(), None).await.err().expect("refused");
        assert_eq!((status, err["error"]["code"].as_str().unwrap()), (400, code), "{body}");
    }
    assert!(stub.requests().is_empty());
}

#[tokio::test]
async fn undeclared_unconfigured_missing_and_foreign_origin_calls_are_refused() {
    let (ts, _, _) = stub_server(false, SampleSettings::default()).await;
    let undeclared = artifact(&ts, json!({"db": {}})).await;
    let (s, e) = start(&ts, &undeclared, json!({"input": "q"}), None).await.err().unwrap();
    assert_eq!((s, e["error"]["code"].as_str().unwrap()), (403, "not_declared"));
    let (s, _) = start(&ts, "zzzzzzzzzzzz", json!({"input": "q"}), None).await.err().unwrap();
    assert_eq!(s, 404);
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/sample", ts.base)))
        .header("cookie", format!("clax_viewer={OWNER_BROWSER}"))
        .header("origin", format!("http://{aid}.localhost:{}", ts.addr.port()))
        .json(&json!({"input": "q"})).send().await.unwrap();
    assert_eq!(res.status(), 403);
    let off = TestServer::spawn().await;
    let aid = artifact(&off, json!({"sample": {}})).await;
    let (s, e) = start(&off, &aid, json!({"input": "q"}), None).await.err().unwrap();
    assert_eq!((s, e["error"]["code"].as_str().unwrap()), (403, "sampling_disabled"));
}

#[tokio::test]
async fn status_reports_availability_and_limits_and_never_the_key() {
    let (ts, _, _) = stub_server(true, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let v: Value = ts.get_authed(&format!("/api/artifacts/{aid}/sample")).await.json().await.unwrap();
    assert_eq!(v["available"], true);
    assert_eq!(v["provider"], "stub");
    assert_eq!(v["limits"]["maxPromptBytes"], 65536);
    assert_eq!(v["limits"]["images"]["maxCount"], 5);
    let off = TestServer::spawn().await;
    let aid = artifact(&off, json!({"sample": {}})).await;
    let v: Value = off.get_authed(&format!("/api/artifacts/{aid}/sample")).await.json().await.unwrap();
    assert_eq!((v["available"].clone(), v["provider"].clone()), (json!(false), Value::Null));
    assert!(v["limits"].get("images").is_none());
    let on = TestServer::spawn_with(|st| {
        st.sample = Arc::new(Sampler::from_config(&Default::default(), |_| Some("sk-never-shown".into())));
    })
    .await;
    let aid = artifact(&on, json!({"sample": {}})).await;
    let text = on.get_authed(&format!("/api/artifacts/{aid}/sample")).await.text().await.unwrap();
    assert!(!text.contains("sk-never-shown"), "{text}");
}

#[tokio::test]
async fn only_the_owners_browser_may_spend_the_key() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let lan = ts.viewer(Some("Ben")).await;
    let url = format!("{}/api/artifacts/{aid}/sample", ts.base);
    // A LAN viewer: a cookie, no token.
    let res = ts.client.post(&url).header("cookie", format!("clax_viewer={}", lan.cookie)).json(&json!({"input": "q"})).send().await.unwrap();
    assert_eq!(res.status(), 401);
    // An agent or a script: the token, no browser.
    let res = ts.authed(ts.client.post(&url)).json(&json!({"input": "q"})).send().await.unwrap();
    assert_eq!(res.status(), 403);
    assert_eq!(res.json::<Value>().await.unwrap()["error"]["code"], "forbidden");
    let res = ts.client.post(format!("{url}/01J00000000000000000000000/tool_result")).header("cookie", format!("clax_viewer={}", lan.cookie)).json(&json!({"id": "x", "content": "y"})).send().await.unwrap();
    assert_eq!(res.status(), 401);
    assert!(stub.requests().is_empty());
    // The status a caller without the token reads says nothing is available.
    let v: Value = ts.get(&format!("/api/artifacts/{aid}/sample")).await.json().await.unwrap();
    assert_eq!((v["available"].clone(), v["provider"].clone(), v["calls_today"].clone()), (json!(false), Value::Null, json!(0)));
}
```

(`bytes` is already a dev-dependency of `clax-server`.)

- [ ] **Step 10: Run them to verify they fail**

Run: `cargo test -p clax-server --test api_sample`
Expected: FAIL (404/405: no sample routes).

- [ ] **Step 11: Implement the routes**

`crates/clax-server/src/routes/sample.rs`:

```rust
//! The `sample` capability's routes (spec §6 "Sample"; `docs/contract.md`
//! "Sample protocol"). All three refuse a foreign `Origin`: pages reach them
//! only through the shell, which asks the viewer's consent first. Only the
//! owner's browser spends the key: the call and tool-result routes need the
//! bearer token (401 `unauthorized`) and a viewer cookie (403 `forbidden`:
//! an agent or a script holding the token is not a browser), and the status
//! route answers `available: false` to a caller without the token. The call
//! route streams `start`, `text`, `tool_call`, and one `done` or `error` over
//! SSE; closing the stream cancels the call.

use crate::auth::{RequireToken, has_token};
use crate::error::ApiError;
use crate::routes::artifacts::parse_id;
use crate::sample::flight::{self, Flight, Out};
use crate::sample::request::{self, MAX_TOOL_RESULT_BYTES, SampleBody};
use crate::sample::{Deliver, ToolOutput};
use crate::state::AppState;
use crate::viewer::{SameOrigin, ViewerCookie};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;

/// Body limit of the call route: five downsized images, base64, with room to spare.
pub const SAMPLE_BODY_LIMIT: usize = 48 * 1024 * 1024;

async fn declared(s: &AppState, aid: &str) -> Result<String, ApiError> {
    let id = parse_id(aid)?;
    let canonical = id.as_str().to_string();
    let artifact = s.store_call(move |st| st.get_artifact(&id)).await?.ok_or_else(ApiError::not_found)?;
    if artifact.capabilities.get("sample").is_none() {
        return Err(ApiError::forbidden("not_declared", "this artifact does not declare sample"));
    }
    Ok(canonical)
}

/// The viewer cookie of the owner's browser making a call; 403 `forbidden`
/// without one.
fn owner_browser(cookie: Option<String>) -> Result<String, ApiError> {
    cookie.ok_or_else(|| ApiError::forbidden("forbidden", "sample() is spent from the owner's browser only"))
}

/// `GET /api/artifacts/{aid}/sample`: whether this daemon can sample for the
/// caller, and the view's limits. Without the token: never available.
pub async fn status(State(s): State<AppState>, Path(aid): Path<String>, _o: SameOrigin, headers: axum::http::HeaderMap) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    s.store_call(move |st| st.get_artifact(&id)).await?.ok_or_else(ApiError::not_found)?;
    if !has_token(&headers, &s.token) {
        return Ok(Json(json!({"available": false, "provider": null, "limits": request::limits_json(false), "calls_today": 0, "daily_call_cap": null})));
    }
    let images = s.sample.provider().is_some_and(|p| p.supports_images());
    Ok(Json(json!({
        "available": s.sample.available(),
        "provider": s.sample.provider_name(),
        "limits": request::limits_json(images),
        "calls_today": 0,
        "daily_call_cap": s.sample.settings.daily_call_cap,
    })))
}

/// The SSE response: `start`, then the flight's frames.
pub(crate) fn stream_response(s: &AppState, start: Value, frames: impl Stream<Item = Out> + Send + 'static) -> Response {
    let first = futures::stream::once(async move { Ok::<_, Infallible>(SseEvent::default().event("start").data(start.to_string())) });
    let rest = frames.map(|o| {
        let (name, data) = o.sse();
        Ok::<_, Infallible>(SseEvent::default().event(name).data(data.to_string()))
    });
    Sse::new(first.chain(rest))
        .keep_alive(KeepAlive::new().interval(s.sse_keep_alive).text("keep-alive"))
        .into_response()
}

/// `POST /api/artifacts/{aid}/sample`.
pub async fn sample(
    State(s): State<AppState>,
    Path(aid): Path<String>,
    _o: SameOrigin,
    _t: RequireToken,
    ViewerCookie(cookie): ViewerCookie,
    body: Result<Json<SampleBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    // `Some` from here on: the per-viewer APIs below take the cookie as an option.
    let cookie = Some(owner_browser(cookie)?);
    let _aid = declared(&s, &aid).await?;
    let Some(provider) = s.sample.provider().cloned() else {
        return Err(ApiError::forbidden("sampling_disabled", "no sample provider is configured on this machine"));
    };
    let Json(body) = body.map_err(|e| ApiError::bad_request("invalid_request", e.body_text()))?;
    let prepared = request::prepare(body, &s.sample.settings, provider.supports_images())?;
    let call_id = clax_core::new_ulid();
    let f = Flight::new();
    s.sample.open_call(&call_id, cookie);
    let task = tokio::spawn(flight::drive(s.sample.clone(), call_id.clone(), prepared, f.clone(), |_| {}));
    f.set_abort(task.abort_handle());
    let start = json!({"call_id": call_id, "cached": false, "calls_today": 0, "daily_call_cap": s.sample.settings.daily_call_cap});
    Ok(stream_response(&s, start, f.reader()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResultBody {
    id: String,
    content: String,
    #[serde(default)]
    is_error: bool,
}

/// `POST /api/artifacts/{aid}/sample/{call}/tool_result`: a page tool's answer.
pub async fn tool_result(
    State(s): State<AppState>,
    Path((aid, call)): Path<(String, String)>,
    _o: SameOrigin,
    _t: RequireToken,
    ViewerCookie(cookie): ViewerCookie,
    body: Result<Json<ToolResultBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let cookie = Some(owner_browser(cookie)?);
    parse_id(&aid)?;
    let Json(b) = body.map_err(|e| ApiError::bad_request("invalid_request", e.body_text()))?;
    if b.content.len() > MAX_TOOL_RESULT_BYTES {
        return Err(ApiError::bad_request("invalid_request", format!("a tool result is at most {MAX_TOOL_RESULT_BYTES} bytes")));
    }
    match s.sample.deliver(&call, cookie.as_deref(), &b.id, ToolOutput { content: b.content, is_error: b.is_error }) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(Deliver::NotFound) => Err(ApiError::not_found()),
        Err(Deliver::Forbidden) => Err(ApiError::forbidden("forbidden", "this call belongs to another viewer")),
    }
}
```

In `crates/clax-server/src/routes/mod.rs`, add `pub mod sample;` and, on the top-level router `r` next to `/api/events` (outside the timeout groups; the stream is long-lived):

```rust
        .route(
            "/api/artifacts/{aid}/sample",
            get(sample::status).post(sample::sample.layer(DefaultBodyLimit::max(sample::SAMPLE_BODY_LIMIT))),
        )
        .route("/api/artifacts/{aid}/sample/{call}/tool_result", post(sample::tool_result))
```

- [ ] **Step 12: Run the route tests to verify they pass**

Run: `cargo test -p clax-server --test api_sample`
Expected: PASS. (`a_tool_that_never_answers_times_out_into_an_error_result` reads "within 0 s" because the test timeout is 200 ms; the message is `as_secs()`.)

- [ ] **Step 13: Write the failing tests for the daemon's sample report and the doctor line**

Append to `crates/clax-server/tests/api_sample.rs`:

```rust
#[tokio::test]
async fn the_daemon_reports_its_sampler_to_the_token_only_and_never_the_key() {
    let ts = TestServer::spawn_with(|st| {
        st.sample = Arc::new(Sampler::from_config(&Default::default(), |_| Some("sk-never-shown".into())));
    })
    .await;
    assert_eq!(ts.get("/api/sample").await.status(), 401);
    let res = ts.get_authed("/api/sample").await;
    let text = res.text().await.unwrap();
    assert!(!text.contains("sk-never-shown"), "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v, json!({"available": true, "provider": "anthropic", "reason": null, "detail": null, "key_env": "ANTHROPIC_API_KEY", "daily_call_cap": null}));
    let off = TestServer::spawn_with(|st| st.sample = Arc::new(Sampler::from_config(&Default::default(), |_| None))).await;
    let v: Value = off.get_authed("/api/sample").await.json().await.unwrap();
    assert_eq!((v["available"].clone(), v["reason"].clone()), (json!(false), json!("no_key")));
}
```

In `crates/clax-cli/src/commands/doctor.rs`'s test module:

```rust
    #[test]
    fn the_sample_line_reports_the_daemon_or_the_file_and_warns_on_a_bad_table() {
        use super::sample_check;
        let on = sample_check(Some(&json!({"available": true, "provider": "anthropic", "reason": null, "detail": null, "key_env": "ANTHROPIC_API_KEY", "daily_call_cap": 200})), None);
        assert_eq!((on["ok"].clone(), on["warn"].clone()), (json!(true), Value::Null));
        assert_eq!(on["detail"], "anthropic, key from ANTHROPIC_API_KEY, at most 200 calls per artifact a day");
        let off = sample_check(Some(&json!({"available": false, "provider": null, "reason": "no_key", "detail": null, "key_env": "ANTHROPIC_API_KEY", "daily_call_cap": null})), None);
        assert_eq!(off["ok"], true);
        assert!(off["detail"].as_str().unwrap().contains("ANTHROPIC_API_KEY is not set in the daemon's environment"));
        let bad = sample_check(Some(&json!({"available": false, "provider": null, "reason": "bad_config", "detail": "config.toml: [sample] provider is \"anthropic\" or \"stub\", not \"openai\"", "key_env": null, "daily_call_cap": null})), None);
        assert_eq!((bad["ok"].clone(), bad["warn"].clone()), (json!(true), json!(true)));
        assert!(bad["detail"].as_str().unwrap().starts_with("sample() is off: config.toml: [sample]"));
        let local = sample_check(None, Some(Err("config.toml: [sample] max_tokens must be at least 1".into())));
        assert_eq!(local["warn"], true);
        let local = sample_check(None, Some(Ok("stub".into())));
        assert_eq!(local["detail"], "stub (no daemon is running)");
    }
```

(`use serde_json::Value;` joins the test module's imports if it lacks it.)

- [ ] **Step 14: Run them to verify they fail**

Run: `cargo test -p clax-server --test api_sample the_daemon_reports && cargo test -p clax-cli --bin clax the_sample_line`
Expected: FAIL (no `/api/sample`; no `sample_check`).

- [ ] **Step 15: Add `GET /api/sample` and the doctor line**

In `crates/clax-server/src/routes/sample.rs`:

```rust
/// `GET /api/sample` (token): whether this daemon samples, and why not.
/// Names the key variable, never its value. `clax doctor` reads it.
pub async fn daemon(State(s): State<AppState>, _t: RequireToken) -> Json<Value> {
    use crate::sample::OffReason;
    let (reason, detail) = match s.sample.reason() {
        None => (None, None),
        Some(OffReason::NoKey) => (Some("no_key"), None),
        Some(OffReason::BadConfig(m)) => (Some("bad_config"), Some(m)),
        Some(OffReason::Disabled) => (Some("disabled"), None),
    };
    Json(json!({
        "available": s.sample.available(),
        "provider": s.sample.provider_name(),
        "reason": reason,
        "detail": detail,
        "key_env": s.sample.key_env(),
        "daily_call_cap": s.sample.settings.daily_call_cap,
    }))
}
```

and in `routes/mod.rs`, inside `api_fast` beside `/api/push`: `.route("/api/sample", get(sample::daemon))`.

In `crates/clax-cli/src/commands/doctor.rs`, a check may now warn: it passes (`ok: true`, so `doctor` still exits 0) and prints `warn` in place of `ok  `:

```rust
/// A passing check that the person should still read: printed `warn`.
fn warn(name: &str, detail: impl Into<String>) -> serde_json::Value {
    serde_json::json!({"name": name, "ok": true, "warn": true, "detail": detail.into()})
}

/// `sample`: the daemon's `GET /api/sample` when one answers, else the home's
/// `[sample]` table read here (`local`: the provider, or why the table is
/// invalid). A bad table warns: the daemon starts with sample() off.
fn sample_check(daemon: Option<&serde_json::Value>, local: Option<Result<String, String>>) -> serde_json::Value {
    if let Some(d) = daemon {
        let key = d["key_env"].as_str().unwrap_or("its key variable");
        return match (d["provider"].as_str(), d["reason"].as_str()) {
            (Some(p), _) => {
                let mut detail = if p == "anthropic" { format!("anthropic, key from {key}") } else { p.to_string() };
                if let Some(cap) = d["daily_call_cap"].as_u64() {
                    detail.push_str(&format!(", at most {cap} calls per artifact a day"));
                }
                check("sample", true, detail)
            }
            (None, Some("bad_config")) => warn("sample", format!("sample() is off: {}", d["detail"].as_str().unwrap_or("config.toml is invalid"))),
            (None, Some("no_key")) => check("sample", true, format!("sample() is off: {key} is not set in the daemon's environment")),
            _ => check("sample", true, "sample() is off"),
        };
    }
    match local {
        Some(Ok(p)) => check("sample", true, format!("{p} (no daemon is running)")),
        Some(Err(e)) => warn("sample", format!("sample() will be off: {e}")),
        None => check("sample", true, "no daemon is running"),
    }
}
```

In `run`, after the `ui` check:

```rust
    let local = clax_core::config::HomeConfig::load(home.root())
        .and_then(|c| c.sample())
        .map(|s| s.provider)
        .map_err(|e| e.to_string());
    checks.push(sample_check(client.as_ref().and_then(|c| c.get("/api/sample").ok()).as_ref(), Some(local)));
```

and in the printer, the status column reads

```rust
                        if c["warn"].as_bool() == Some(true) {
                            "warn"
                        } else if c["ok"].as_bool().unwrap() {
                            "ok  "
                        } else {
                            "FAIL"
                        },
```

- [ ] **Step 16: Run everything and stage**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: PASS.

Stage the change (`git add crates/clax-server crates/clax-cli`); do not commit. The controller commits it with the message:

```text
Stream sample calls from the owner's browser over SSE with page tool rounds, cancellation on disconnect, the contract's error codes, and a doctor line
```


---

### Task 4: Answer caching, the per-viewer queue, and the daily cap

Carried over from the earlier draft of this plan, renamed. Every request in these tests is the owner's browser (Task 3's `start` sends the token and a cookie); "per viewer" means per browser of the owner.

**Files:**
- Create: `crates/clax-server/src/sample/cache.rs`, `crates/clax-server/src/sample/quota.rs`
- Modify: `crates/clax-server/src/sample/mod.rs` (modules; `cache`, `counts`, `queues` on `Sampler`), `crates/clax-server/src/routes/sample.rs` (replay, share, queue, cap, counts)
- Test: unit tests in `cache.rs` and `quota.rs`; `crates/clax-server/tests/api_sample.rs` (appended)

**Interfaces:**
- Consumes (Task 3): `Prepared.{cache, input_key}`, `CachePolicy`, `Flight`, `Out`, `Done`, `drive` and its `on_finish`, `stream_response`, the test helpers of `api_sample.rs` (`stub_server`, `stub_server_with`, `artifact`, `start`, `Call`, `text_of`).
- Produces:
  - `sample::cache::AnswerCache` (`Default`): `AnswerCache::key(aid: &str, viewer: Option<&str>, input_key: &str) -> String`, `get(&self, key: &str, gc: Duration, now: Instant) -> Option<Done>`, `put(&self, key: String, done: Done, gc: Duration, now: Instant)`, `flight(&self, key: &str) -> Option<Arc<Flight>>`, `begin(&self, key: String, f: Arc<Flight>)`, `end(&self, key: &str, f: &Arc<Flight>)`.
  - `sample::quota::{CallCounts, ViewerQueues, RUNNING_PER_VIEWER = 2, WAITING_PER_VIEWER = 4}`: `CallCounts::today(&self, aid: &str, date: NaiveDate) -> u32`, `CallCounts::try_take(&self, aid: &str, date: NaiveDate, cap: Option<u32>) -> Result<u32, u32>`; `ViewerQueues::enter(&self, viewer: &str) -> Option<OwnedSemaphorePermit>` (async; `None` when the viewer already has two running and four waiting).
  - `Sampler.{cache, counts, queues}` (public fields); `routes::sample::today() -> NaiveDate`.
  - The `start` frame's `cached`, `calls_today`, `daily_call_cap` and the status route's `calls_today` become real.

- [ ] **Step 1: Write the failing unit tests**

`crates/clax-server/src/sample/cache.rs`, tests only (add `pub mod cache; pub mod quota;` to `sample/mod.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::request::Tier;

    fn done(t: &str) -> Done {
        Done { text: t.into(), truncated: false, model_tier_applied: Tier::Default, value: None }
    }

    #[test]
    fn an_answer_replays_while_younger_than_both_windows() {
        let c = AnswerCache::default();
        let t0 = Instant::now();
        let k = AnswerCache::key("7q3k9mzx2b4t", Some("01J0"), "text|default|\"q\"|0");
        c.put(k.clone(), done("a"), Duration::from_secs(60), t0);
        assert_eq!(c.get(&k, Duration::from_secs(300), t0 + Duration::from_secs(59)), Some(done("a")));
        assert_eq!(c.get(&k, Duration::from_secs(300), t0 + Duration::from_secs(60)), None);
        assert_eq!(c.get(&k, Duration::from_secs(10), t0 + Duration::from_secs(11)), None);
    }

    #[test]
    fn keys_separate_artifacts_and_viewers() {
        let a = AnswerCache::key("7q3k9mzx2b4t", Some("v1"), "k");
        assert_ne!(a, AnswerCache::key("7q3k9mzx2b4t", Some("v2"), "k"));
        assert_ne!(a, AnswerCache::key("zzzzzzzzzzzz", Some("v1"), "k"));
        assert_ne!(AnswerCache::key("7q3k9mzx2b4t", None, "k"), AnswerCache::key("7q3k9mzx2b4t", Some("-"), "k"));
    }

    #[test]
    fn flights_are_shared_until_they_end() {
        let c = AnswerCache::default();
        let f = Flight::new();
        c.begin("k".into(), f.clone());
        assert!(Arc::ptr_eq(&c.flight("k").unwrap(), &f));
        let newer = Flight::new();
        c.begin("k".into(), newer.clone());
        c.end("k", &f);
        assert!(Arc::ptr_eq(&c.flight("k").unwrap(), &newer));
        c.end("k", &newer);
        assert!(c.flight("k").is_none());
    }
}
```

`crates/clax-server/src/sample/quota.rs`, tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_per_artifact_per_day_and_stop_at_the_cap() {
        let c = CallCounts::default();
        let d1 = NaiveDate::from_ymd_opt(2026, 9, 29).unwrap();
        let d2 = d1.succ_opt().unwrap();
        assert_eq!(c.try_take("a", d1, Some(2)), Ok(1));
        assert_eq!(c.try_take("a", d1, Some(2)), Ok(2));
        assert_eq!(c.try_take("a", d1, Some(2)), Err(2));
        assert_eq!(c.today("a", d1), 2);
        assert_eq!(c.try_take("b", d1, Some(2)), Ok(1));
        assert_eq!(c.today("a", d2), 0);
        assert_eq!(c.try_take("a", d2, Some(2)), Ok(1));
        assert_eq!(c.try_take("a", d2, None), Ok(2));
        assert_eq!(c.try_take("z", d1, Some(0)), Err(0));
    }

    #[tokio::test]
    async fn two_run_four_wait_and_the_rest_are_refused() {
        let q = Arc::new(ViewerQueues::default());
        let a = q.enter("v").await.unwrap();
        let _b = q.enter("v").await.unwrap();
        let mut waiters = Vec::new();
        for _ in 0..WAITING_PER_VIEWER {
            let q = q.clone();
            waiters.push(tokio::spawn(async move { q.enter("v").await.is_some() }));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(q.enter("v").await.is_none());
        assert!(q.enter("w").await.is_some());
        drop(a);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(waiters.iter().filter(|w| w.is_finished()).count() >= 1);
        for w in &waiters {
            w.abort();
        }
    }

    #[tokio::test]
    async fn a_waiter_that_gives_up_frees_its_place() {
        let q = Arc::new(ViewerQueues::default());
        let _a = q.enter("v").await.unwrap();
        let _b = q.enter("v").await.unwrap();
        let mut waiters: Vec<_> = (0..WAITING_PER_VIEWER)
            .map(|_| { let q = q.clone(); tokio::spawn(async move { q.enter("v").await.is_some() }) })
            .collect();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        waiters.pop().unwrap().abort();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let q2 = q.clone();
        let again = tokio::spawn(async move { q2.enter("v").await.is_some() });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!again.is_finished(), "the freed place is taken by a waiter, not refused");
        again.abort();
        for w in &waiters {
            w.abort();
        }
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p clax-server --lib sample::cache sample::quota`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the cache and the quotas**

`crates/clax-server/src/sample/cache.rs`, above its tests:

```rust
//! Answer caching for `sample()` (`sample.d.ts` `cache`): per artifact, per
//! viewer, per question (verb, tier, every turn, the images), in the daemon's
//! memory. A stored answer is replayed while it is younger than both the
//! window it was stored with and the window of the call asking now. An
//! identical call made while the first is still running follows the first's
//! flight instead of asking again. Only successful answers are stored.

use super::flight::{Done, Flight};
use super::request::MAX_GC;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Stored {
    done: Done,
    at: Instant,
    gc: Duration,
}

#[derive(Default)]
pub struct AnswerCache {
    entries: Mutex<HashMap<String, Stored>>,
    flights: Mutex<HashMap<String, Arc<Flight>>>,
}

impl AnswerCache {
    /// The cache key of a question asked by `viewer` (the viewer cookie, kept
    /// in memory only; `None` for a cookieless caller) in artifact `aid`.
    pub fn key(aid: &str, viewer: Option<&str>, input_key: &str) -> String {
        match viewer {
            Some(v) => format!("{aid}|v:{v}|{input_key}"),
            None => format!("{aid}|anon|{input_key}"),
        }
    }

    pub fn get(&self, key: &str, gc: Duration, now: Instant) -> Option<Done> {
        let entries = self.entries.lock().expect("cache lock");
        let e = entries.get(key)?;
        (now.saturating_duration_since(e.at) < gc.min(e.gc)).then(|| e.done.clone())
    }

    /// Stores (or overwrites) an answer; entries older than a day are dropped.
    pub fn put(&self, key: String, done: Done, gc: Duration, now: Instant) {
        let mut entries = self.entries.lock().expect("cache lock");
        entries.retain(|_, e| now.saturating_duration_since(e.at) < MAX_GC);
        entries.insert(key, Stored { done, at: now, gc });
    }

    /// The running flight for `key`, if one is still unfinished.
    pub fn flight(&self, key: &str) -> Option<Arc<Flight>> {
        self.flights.lock().expect("flights lock").get(key).filter(|f| !f.is_finished()).cloned()
    }

    /// Makes `f` the flight later identical calls follow.
    pub fn begin(&self, key: String, f: Arc<Flight>) {
        self.flights.lock().expect("flights lock").insert(key, f);
    }

    /// Stops sharing `f` (only if it is still the flight for `key`).
    pub fn end(&self, key: &str, f: &Arc<Flight>) {
        let mut flights = self.flights.lock().expect("flights lock");
        if flights.get(key).is_some_and(|cur| Arc::ptr_eq(cur, f)) {
            flights.remove(key);
        }
    }
}
```

`crates/clax-server/src/sample/quota.rs`, above its tests:

```rust
//! Limits on `sample()` spending (spec §14, §18; `sample.d.ts`): a count of
//! calls that reached the provider per artifact per local day, stopped at
//! `daily_call_cap`, and a per-viewer queue in which two calls run, four more
//! wait their turn, and the rest are refused `rate_limited`. Counts live in
//! memory and restart with the daemon.

use chrono::NaiveDate;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const RUNNING_PER_VIEWER: usize = 2;
pub const WAITING_PER_VIEWER: usize = 4;

#[derive(Default)]
pub struct CallCounts {
    days: Mutex<HashMap<String, (NaiveDate, u32)>>,
}

impl CallCounts {
    /// Calls counted for `aid` on `date`.
    pub fn today(&self, aid: &str, date: NaiveDate) -> u32 {
        match self.days.lock().expect("counts lock").get(aid) {
            Some((d, n)) if *d == date => *n,
            _ => 0,
        }
    }

    /// Counts one more call for `aid` on `date` and returns the new count, or
    /// `Err(cap)` without counting when the count has reached `cap`.
    pub fn try_take(&self, aid: &str, date: NaiveDate, cap: Option<u32>) -> Result<u32, u32> {
        let mut days = self.days.lock().expect("counts lock");
        let entry = days.entry(aid.to_string()).or_insert((date, 0));
        if entry.0 != date {
            *entry = (date, 0);
        }
        if let Some(c) = cap
            && entry.1 >= c
        {
            return Err(c);
        }
        entry.1 += 1;
        Ok(entry.1)
    }
}

struct Queue {
    running: Arc<Semaphore>,
    waiting: AtomicUsize,
}

#[derive(Default)]
pub struct ViewerQueues {
    queues: Mutex<HashMap<String, Arc<Queue>>>,
}

struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ViewerQueues {
    /// A running slot for `viewer`, waiting for one if two are taken; `None`
    /// at once when four calls already wait. Dropping the future gives up the place.
    pub async fn enter(&self, viewer: &str) -> Option<OwnedSemaphorePermit> {
        let q = self
            .queues
            .lock()
            .expect("queues lock")
            .entry(viewer.to_string())
            .or_insert_with(|| Arc::new(Queue { running: Arc::new(Semaphore::new(RUNNING_PER_VIEWER)), waiting: AtomicUsize::new(0) }))
            .clone();
        if let Ok(p) = q.running.clone().try_acquire_owned() {
            return Some(p);
        }
        if q.waiting.fetch_add(1, Ordering::SeqCst) >= WAITING_PER_VIEWER {
            q.waiting.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        let _place = Waiting(&q.waiting);
        q.running.clone().acquire_owned().await.ok()
    }
}
```

Add to `Sampler` (initialised `Default::default()` in `Sampler::build`):

```rust
    /// Stored answers and running flights, per viewer and artifact.
    pub cache: cache::AnswerCache,
    /// Calls per artifact per day.
    pub counts: quota::CallCounts,
    /// Running and waiting calls per viewer.
    pub queues: quota::ViewerQueues,
```

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -p clax-server --lib sample::cache sample::quota`
Expected: PASS.

- [ ] **Step 5: Write the failing route tests**

Append to `crates/clax-server/tests/api_sample.rs`:

```rust
async fn done_of(ts: &TestServer, aid: &str, body: Value, cookie: Option<&str>) -> (Value, Vec<(String, Value)>) {
    let mut c = start(ts, aid, body, cookie).await.expect("a stream");
    let started = c.next().await.1;
    (started, c.rest().await)
}

#[tokio::test]
async fn a_repeat_is_replayed_without_asking_the_provider() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let v = ts.viewer(None).await;
    let (s1, f1) = done_of(&ts, &aid, json!({"input": "hello"}), Some(&v.cookie)).await;
    let (s2, f2) = done_of(&ts, &aid, json!({"input": "hello"}), Some(&v.cookie)).await;
    assert_eq!((s1["cached"].clone(), s2["cached"].clone()), (json!(false), json!(true)));
    assert_eq!(f2, vec![("done".to_string(), f1.last().unwrap().1.clone())]);
    assert_eq!(stub.requests().len(), 1);
    done_of(&ts, &aid, json!({"input": "hello", "model_tier": "quick"}), Some(&v.cookie)).await;
    done_of(&ts, &aid, json!({"input": "hello", "verb": "json"}), Some(&v.cookie)).await;
    let (s, _) = done_of(&ts, &aid, json!({"input": "hello", "cache": false}), Some(&v.cookie)).await;
    assert_eq!(s["cached"], false);
    assert_eq!(stub.requests().len(), 4);
    let (s, _) = done_of(&ts, &aid, json!({"input": "hello"}), Some(&v.cookie)).await;
    assert_eq!(s["cached"], true);
}

#[tokio::test]
async fn answers_are_cached_per_viewer() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let (owner, lan) = (ts.viewer(Some("Owner")).await, ts.viewer(None).await); // two of the owner's browsers
    done_of(&ts, &aid, json!({"input": "same question"}), Some(&owner.cookie)).await;
    let (s, _) = done_of(&ts, &aid, json!({"input": "same question"}), Some(&lan.cookie)).await;
    assert_eq!(s["cached"], false);
    let (s, _) = done_of(&ts, &aid, json!({"input": "same question"}), None).await;
    assert_eq!(s["cached"], false);
    assert_eq!(stub.requests().len(), 3);
    let other = artifact(&ts, json!({"sample": {}})).await;
    let (s, _) = done_of(&ts, &other, json!({"input": "same question"}), Some(&owner.cookie)).await;
    assert_eq!(s["cached"], false);
}

#[tokio::test]
async fn a_short_window_expires_and_refresh_asks_again_and_overwrites() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    done_of(&ts, &aid, json!({"input": "q", "cache": {"gc_time_ms": 50}}), None).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (s, _) = done_of(&ts, &aid, json!({"input": "q"}), None).await;
    assert_eq!(s["cached"], false);
    let (s, _) = done_of(&ts, &aid, json!({"input": "q", "cache": {"gc_time_ms": 60000, "refresh": true}}), None).await;
    assert_eq!(s["cached"], false);
    let (s, _) = done_of(&ts, &aid, json!({"input": "q", "cache": {"gc_time_ms": 60000}}), None).await;
    assert_eq!(s["cached"], true);
    assert_eq!(stub.requests().len(), 3);
}

#[tokio::test]
async fn failures_and_invalid_json_are_never_stored() {
    let (ts, stub, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    for body in [json!({"input": "[[error:upstream_error]]"}), json!({"input": "[[say:nope]]", "verb": "json"})] {
        for _ in 0..2 {
            let (s, _) = done_of(&ts, &aid, body.clone(), None).await;
            assert_eq!(s["cached"], false, "{body}");
        }
    }
    assert_eq!(stub.requests().len(), 4);
}

#[tokio::test]
async fn identical_calls_in_flight_share_one_answer_even_if_the_first_leaves() {
    let (ts, stub, _) = stub_server_with(false, SampleSettings::default(), Duration::from_millis(200)).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let mut first = start(&ts, &aid, json!({"input": "shared"}), None).await.unwrap();
    first.next().await;
    first.next().await;
    let mut second = start(&ts, &aid, json!({"input": "shared"}), None).await.unwrap();
    assert_eq!(second.next().await.1["cached"], true);
    drop(first);
    let frames = second.rest().await;
    assert_eq!(text_of(&frames), "echo: shared");
    assert_eq!(frames.last().unwrap().1["text"], "echo: shared");
    assert_eq!(stub.requests().len(), 1);
}

#[tokio::test]
async fn the_daily_cap_counts_calls_that_reach_the_provider() {
    let settings = SampleSettings { daily_call_cap: Some(2), ..SampleSettings::default() };
    let (ts, _, _) = stub_server(false, settings).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let (s, _) = done_of(&ts, &aid, json!({"input": "a"}), None).await;
    assert_eq!((s["calls_today"].clone(), s["daily_call_cap"].clone()), (json!(1), json!(2)));
    let (s, _) = done_of(&ts, &aid, json!({"input": "b", "cache": false}), None).await;
    assert_eq!(s["calls_today"], 2);
    let (status, e) = start(&ts, &aid, json!({"input": "c"}), None).await.err().expect("capped");
    assert_eq!((status, e["error"]["code"].as_str().unwrap()), (429, "rate_limited"));
    let (s, _) = done_of(&ts, &aid, json!({"input": "a"}), None).await;
    assert_eq!((s["cached"].clone(), s["calls_today"].clone()), (json!(true), json!(2)));
    let v: Value = ts.get_authed(&format!("/api/artifacts/{aid}/sample")).await.json().await.unwrap();
    assert_eq!((v["calls_today"].clone(), v["daily_call_cap"].clone()), (json!(2), json!(2)));
    let other = artifact(&ts, json!({"sample": {}})).await;
    let (s, _) = done_of(&ts, &other, json!({"input": "c"}), None).await;
    assert_eq!(s["calls_today"], 1);
}

#[tokio::test]
async fn a_flood_from_one_viewer_is_rate_limited() {
    let (ts, _, _) = stub_server(false, SampleSettings::default()).await;
    let aid = artifact(&ts, json!({"sample": {}})).await;
    let (flooder, other) = (ts.viewer(None).await, ts.viewer(None).await);
    let url = format!("{}/api/artifacts/{aid}/sample", ts.base);
    let mut held = Vec::new();
    for _ in 0..6 {
        let (client, url, cookie, token) = (ts.client.clone(), url.clone(), flooder.cookie.clone(), ts.token.clone());
        held.push(tokio::spawn(async move {
            let res = client.post(url).bearer_auth(token).header("cookie", format!("clax_viewer={cookie}")).json(&json!({"input": "[[slow]]", "cache": false})).send().await;
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(res);
        }));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (status, e) = start(&ts, &aid, json!({"input": "[[slow]]", "cache": false}), Some(&flooder.cookie)).await.err().expect("refused");
    assert_eq!((status, e["error"]["code"].as_str().unwrap()), (429, "rate_limited"));
    let mut fine = start(&ts, &aid, json!({"input": "hi", "cache": false}), Some(&other.cookie)).await.expect("another viewer still runs");
    assert_eq!(fine.next().await.0, "start");
    for h in &held {
        h.abort();
    }
}
```

- [ ] **Step 6: Run them to verify they fail**

Run: `cargo test -p clax-server --test api_sample`
Expected: FAIL (every repeat asks the provider; nothing is capped or queued; `calls_today` is 0).

- [ ] **Step 7: Put caching, the queue, and the cap in the call route**

In `crates/clax-server/src/routes/sample.rs`, add the imports

```rust
use crate::sample::cache::AnswerCache;
use crate::sample::flight::Done;
use crate::sample::request::CachePolicy;
use chrono::NaiveDate;
use std::time::Instant;
```

these helpers

```rust
/// The daemon's local date, for the daily cap.
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

fn rate_limited(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message)
}

fn start_json(call_id: &str, cached: bool, calls_today: u32, cap: Option<u32>) -> Value {
    json!({"call_id": call_id, "cached": cached, "calls_today": calls_today, "daily_call_cap": cap})
}
```

make `status` answer `"calls_today": s.sample.counts.today(id.as_str(), today())` to a caller with the token (take `id.as_str().to_string()` before moving `id` into `store_call`; the tokenless answer keeps `0`), and change `let _aid = declared(&s, &aid).await?;` to `let aid = declared(&s, &aid).await?;` (the canonical ID), and replace the body of `sample` after `let prepared = …?;` with:

```rust
    let cap = s.sample.settings.daily_call_cap;
    let key = AnswerCache::key(&aid, cookie.as_deref(), &prepared.input_key);
    if let CachePolicy::Window { gc, refresh: false } = prepared.cache {
        if let Some(done) = s.sample.cache.get(&key, gc, Instant::now()) {
            let start = start_json(&clax_core::new_ulid(), true, s.sample.counts.today(&aid, today()), cap);
            return Ok(stream_response(&s, start, futures::stream::iter([Out::Done(done)])));
        }
        if let Some(f) = s.sample.cache.flight(&key) {
            let start = start_json(&clax_core::new_ulid(), true, s.sample.counts.today(&aid, today()), cap);
            return Ok(stream_response(&s, start, f.reader()));
        }
    }
    let permit = s
        .sample
        .queues
        .enter(cookie.as_deref().unwrap_or("anonymous"))
        .await
        .ok_or_else(|| rate_limited("too many calls from this viewer at once; try again when one has finished"))?;
    let calls_today = s
        .sample
        .counts
        .try_take(&aid, today(), cap)
        .map_err(|n| rate_limited(format!("this artifact reached its daily cap of {n} calls")))?;
    let call_id = clax_core::new_ulid();
    let f = Flight::new();
    s.sample.open_call(&call_id, cookie);
    let on_finish: Box<dyn FnOnce(Option<&Done>) + Send> = match prepared.cache {
        CachePolicy::Window { gc, .. } => {
            s.sample.cache.begin(key.clone(), f.clone());
            let (sampler, flight) = (s.sample.clone(), f.clone());
            Box::new(move |done: Option<&Done>| {
                if let Some(d) = done {
                    sampler.cache.put(key.clone(), d.clone(), gc, Instant::now());
                }
                sampler.cache.end(&key, &flight);
            })
        }
        CachePolicy::Off => Box::new(|_: Option<&Done>| {}),
    };
    let (sampler, id, flight) = (s.sample.clone(), call_id.clone(), f.clone());
    let task = tokio::spawn(async move {
        let _running = permit;
        flight::drive(sampler, id, prepared, flight, on_finish).await;
    });
    f.set_abort(task.abort_handle());
    Ok(stream_response(&s, start_json(&call_id, false, calls_today, cap), f.reader()))
```

Extend the route's doc comment: a repeat within the cache window replays the stored answer (`start.cached: true`, one `done`, nothing counted); an identical call made while the first runs follows it (`cached: true`); a viewer runs two calls at once and four wait, the rest are refused 429 `rate_limited`, as is a call past `daily_call_cap`.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p clax-server --test api_sample && cargo test -p clax-server --lib sample`
Expected: PASS.

- [ ] **Step 9: Run everything and stage**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: PASS.

Stage the change (`git add crates/clax-server`); do not commit. The controller commits it with the message:

```text
Cache sample answers per viewer, share identical calls in flight, queue calls per viewer, and enforce the daily cap
```


---

### Task 5: Per-capability lazy parts in the bridge, and `room`'s page side

The bridge's `caps` part loads on the first granted `use()` of any capability, so putting `room` and `sample` in it would make every `db` or `comments` page download both, and would take `partCaps` over its budget (scan B2). Owner ruling: `room` and `sample` are lazy parts of their own, with no budget raised. This task adds the part machinery for any number of parts, the `room` part, and the shell-side names for its failure; Task 7 adds the `sample` part the same way. A part capability's module builds the whole frozen namespace itself, so its member list lives in the part, not in the eager bridge (scan F5): the eager bridge only learns the two names.

The page-side `room` module is carried over from the earlier draft of this plan, renamed, with `prototype` refused as a presence key (scan F13) and a `roomNamespace` entry point for the part.

**Files:**
- Create: `web/bridge/src/caps/room.ts`, `web/bridge/src/parts/room.ts`
- Modify: `web/bridge/src/capabilities.ts` (`PART_CAPABILITIES`, `PartCapabilityName`, `UsableName`, `isPartCapability`; `isCapabilityName` covers both), `web/bridge/src/use.ts` (a part capability's namespace is used as the part built it), `web/bridge/src/bridge.ts` (the `room` part, the name dispatch), `web/bridge/src/parts/types.ts` (`RoomPart`; `Parts.room`), `web/bridge/src/parts-url.ts` (one URL per part from `__CLAX_PARTS__`), `web/bridge/src/parts-static.ts` (`room`), `web/bridge/src/protocol.ts` (`clax:degraded.part` gains `room` and `sample`), `web/scripts/build-parts.mjs` (`PARTS`), `web/vite.bridge.config.ts` (the required names), `web/scripts/bundle-size.mjs` (`partRoom`), `web/perf/bundle-budget.json` (`partRoom`), `web/shell/src/failure.ts` (`PART_FAILED.room`, `PART_FAILED.sample`)
- Test: `web/bridge/test/room.test.ts` (new); `web/bridge/test/capabilities.test.ts`, `web/bridge/test/use.test.ts`, `web/bridge/test/bridge-degraded.test.ts` (updated); `web/e2e/bridge-parts.spec.ts` (updated)

**Interfaces:**
- Consumes: `Rpc.{call, on}`, `CapabilityError`, `CAPABILITY_METHODS`, `buildNamespace`, `Local`, `retrying`, `Parts`, `__CLAX_PARTS__`; the Room relay of the Shared contract.
- Produces:
  - `capabilities.ts`: `PART_CAPABILITIES = ["room", "sample"] as const`, `type PartCapabilityName`, `type UsableName = CapabilityName | PartCapabilityName`, `isPartCapability(name: string): name is PartCapabilityName`; `isCapabilityName(name: string): name is UsableName`.
  - `makeUse(opts: { framed: boolean; rpc: Rpc; locals: (name: UsableName, rpc: Rpc, config: unknown) => Promise<unknown> })`: for a part capability, `locals` resolves the frozen namespace itself.
  - `caps/room.ts`: `makeRoom(rpc: Pick<Rpc, "call" | "on">)`, `roomNamespace(rpc): Readonly<Record<string, unknown>>`, `ROOM_METHODS`, constants `TOPIC`, `ROOM_NAME`, `MAX_JSON_BYTES = 4096`, `MAX_DEPTH = 8`, `MAX_JOINED = 16`, `PRESENCE_INTERVAL_MS = 33`, `FLUSH_FALLBACK_MS = 50`, `JOIN_TIMEOUT_MS = 10_000`; types `WirePeer`, `Peer`, `PeersChange`.
  - `parts/room.ts` exports `roomNamespace` and `ROOM_METHODS`; `type RoomPart = typeof import("./room")`; `Parts.room(attempt?)`.
  - `clax:degraded.part: "comment" | "clip" | "caps" | "room" | "sample"`; `PART_FAILED.room`, `PART_FAILED.sample`.
  - `bundle-budget.json` key `partRoom`; `bundle-size.mjs` measures it.

- [ ] **Step 1: Write the failing bridge tests**

`web/bridge/test/room.test.ts`:

```ts
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_JOINED, PRESENCE_INTERVAL_MS, makeRoom, roomNamespace, type PeersChange, type WirePeer } from "../src/caps/room";
import { CapabilityError } from "../src/rpc";

type Listener = (d: unknown) => void;

function fakeRpc(answer: (method: string, args: unknown[]) => unknown = () => null) {
  const listeners = new Map<string, Set<Listener>>();
  const calls: { method: string; args: unknown[] }[] = [];
  return {
    calls,
    push(topic: string, data: unknown) { for (const f of [...(listeners.get(topic) ?? [])]) f(data); },
    rpc: {
      call: vi.fn(async (_ns: string, method: string, args: unknown[]) => { calls.push({ method, args }); return answer(method, args); }),
      on: (_ns: string, topic: string, f: Listener) => {
        if (!listeners.has(topic)) listeners.set(topic, new Set());
        listeners.get(topic)!.add(f);
        return () => { listeners.get(topic)!.delete(f); };
      },
    },
  };
}

const peer = (label: string, over: Partial<WirePeer> = {}): WirePeer =>
  ({ peer: label, by: null, isMe: false, sameTab: false, kind: "viewer", guest: false, presence: {}, ...over });
const ME = "mmmmmmmmmmmmmmmm";
const OTHER = "oooooooooooooooo";

type Room = {
  emit(topic: unknown, data?: unknown): Promise<void>;
  on(topic: unknown, fn: unknown, onError?: (e: { code: string }) => void): () => void;
  presence(patch: unknown): Promise<void>;
  peers(): readonly { peer: string; presence: Record<string, unknown>; updatedAt: number }[];
  onPeers(fn: (c: PeersChange) => void, onError?: (e: { code: string }) => void): () => void;
  join(name: unknown): Promise<Record<string, (...a: unknown[]) => unknown> & { name: string }>;
  connected(): boolean;
  onConnection(fn: (c: boolean) => void, onError?: (e: { code: string }) => void): () => void;
  sendToClaudeSession(data: unknown): Promise<unknown>;
  canSendToClaudeSession(): Promise<string>;
};

function setup(answer?: (method: string, args: unknown[]) => unknown) {
  const f = fakeRpc(answer);
  const room = makeRoom(f.rpc as never) as unknown as Room;
  const up = () => {
    f.push("connection", { connected: true });
    f.push("welcome", { peer: ME });
    f.push("peers", { room: null, peers: [peer(ME, { isMe: true, sameTab: true })] });
  };
  return { ...f, room, up };
}

describe("room", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("asks the shell to connect once, as soon as the namespace exists", () => {
    const { calls } = setup();
    expect(calls).toEqual([{ method: "connect", args: [] }]);
  });

  it("peers() is a frozen empty array until the first answer, then the same frozen snapshot until something changes", () => {
    const { room, up, push } = setup();
    const empty = room.peers();
    expect(empty).toEqual([]);
    expect(Object.isFrozen(empty)).toBe(true);
    expect(room.peers()).toBe(empty);
    up();
    const one = room.peers();
    expect(one.map(p => p.peer)).toEqual([ME]);
    expect(Object.isFrozen(one) && Object.isFrozen(one[0]) && Object.isFrozen(one[0].presence)).toBe(true);
    expect(room.peers()).toBe(one);
    push("peer", { room: null, peer: peer(OTHER) });
    const two = room.peers();
    expect(two).not.toBe(one);
    expect(two.find(p => p.peer === ME)).toBe(one[0]);
  });

  it("onPeers first presents the room as joined, later only net changes, at most once per frame", async () => {
    const { room, up, push } = setup();
    const seen: PeersChange[] = [];
    room.onPeers(c => seen.push(c));
    await vi.advanceTimersByTimeAsync(100);
    expect(seen).toEqual([]);
    up();
    push("peer", { room: null, peer: peer(OTHER) });
    expect(seen).toEqual([]);
    await vi.advanceTimersByTimeAsync(60);
    expect(seen).toHaveLength(1);
    expect(seen[0].joined.map(p => p.peer).sort()).toEqual([ME, OTHER].sort());
    expect(seen[0].peers).toBe(room.peers());
    push("peer", { room: null, peer: peer(OTHER, { presence: { n: 1 } }) });
    push("peer", { room: null, peer: peer(OTHER, { presence: { n: 2 } }) });
    push("peer", { room: null, peer: peer("jjjjjjjjjjjjjjjj") });
    push("left", { room: null, peer: "jjjjjjjjjjjjjjjj" });
    await vi.advanceTimersByTimeAsync(60);
    expect(seen).toHaveLength(2);
    expect(seen[1].updated.map(p => p.presence)).toEqual([{ n: 2 }]);
    expect(seen[1].joined).toEqual([]);
    expect(seen[1].left).toEqual([]);
    push("left", { room: null, peer: OTHER });
    await vi.advanceTimersByTimeAsync(60);
    expect(seen[2].left.map(p => p.peer)).toEqual([OTHER]);
  });

  it("a peers frame replaces the room: peers missing from it are reported left", async () => {
    const { room, up, push } = setup();
    up();
    push("peer", { room: null, peer: peer(OTHER) });
    const seen: PeersChange[] = [];
    room.onPeers(c => seen.push(c));
    await vi.advanceTimersByTimeAsync(60);
    push("peers", { room: null, peers: [peer(ME, { isMe: true, sameTab: true })] });
    await vi.advanceTimersByTimeAsync(60);
    expect(seen.at(-1)!.left.map(p => p.peer)).toEqual([OTHER]);
  });

  it("presence merges locally at once, removes null fields, and is sent whole, coalesced", async () => {
    const { room, up, calls } = setup();
    up();
    await room.presence({ x: 1, y: 2 });
    await room.presence({ y: null, z: "a" });
    expect(room.peers().find(p => p.peer === ME)!.presence).toEqual({ x: 1, z: "a" });
    expect(calls.filter(c => c.method === "presence")).toEqual([]);
    await vi.advanceTimersByTimeAsync(PRESENCE_INTERVAL_MS);
    expect(calls.filter(c => c.method === "presence")).toEqual([{ method: "presence", args: [null, { x: 1, z: "a" }] }]);
  });

  it("presence refuses bad keys and oversized objects without applying them", async () => {
    const { room, up } = setup();
    up();
    await room.presence({ keep: 1 });
    for (const bad of [{ "1a": 1 }, { "a b": 1 }, { constructor: 1 }, { prototype: 1 }, { ["__proto__"]: 1 }, { big: "x".repeat(5000) }, [1], "x"]) {
      await expect(room.presence(bad)).rejects.toMatchObject({ code: "invalid_argument" });
    }
    expect(room.peers().find(p => p.peer === ME)!.presence).toEqual({ keep: 1 });
  });

  it("emit checks its arguments, drops silently while disconnected, and passes the shell's refusal through", async () => {
    const { room, up, calls } = setup((method, args) => {
      if (method === "emit" && args[1] === "clear") throw new CapabilityError("not_permitted", "admin only");
      return { dropped: false };
    });
    await expect(room.emit("Bad:topic")).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(room.emit("ok", { big: "x".repeat(5000) })).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(room.emit("ok", { f: () => 1 })).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(room.emit("reaction")).resolves.toBeUndefined();
    expect(calls.some(c => c.method === "emit")).toBe(false);
    up();
    await room.emit("reaction", { kind: "wave" });
    expect(calls.at(-1)).toEqual({ method: "emit", args: [null, "reaction", { kind: "wave" }] });
    await room.emit("bare");
    expect(calls.at(-1)).toEqual({ method: "emit", args: [null, "bare"] });
    await expect(room.emit("clear")).rejects.toMatchObject({ code: "not_permitted" });
  });

  it("reports emits the daemon dropped once per page load", async () => {
    const report = vi.fn();
    vi.stubGlobal("reportError", report);
    const { room, up } = setup(() => ({ dropped: true }));
    up();
    await room.emit("a");
    await room.emit("a");
    expect(report).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });

  it("on: TypeError for a non-function, a malformed topic errors once on a microtask, messages reach their topic", async () => {
    const { room, up, push } = setup();
    expect(() => room.on("a", 42)).toThrow(TypeError);
    const onError = vi.fn();
    room.on("Bad:topic", () => {}, onError);
    expect(onError).not.toHaveBeenCalled();
    await Promise.resolve();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0][0]).toMatchObject({ code: "invalid_argument" });
    up();
    const got: unknown[] = [];
    const off = room.on("reaction", m => got.push(m));
    const msg = { ...peer(OTHER), topic: "reaction", data: { k: 1 } };
    push("msg", { room: null, msg: { ...msg, presence: undefined } });
    push("msg", { room: null, msg: { ...msg, topic: "other" } });
    off();
    off();
    push("msg", { room: null, msg });
    expect(got).toHaveLength(1);
    expect(got[0]).toMatchObject({ peer: OTHER, topic: "reaction", data: { k: 1 } });
  });

  it("join returns one object per name, leaves cleanly, and stops at 16", async () => {
    const { room, up, push, calls } = setup();
    up();
    await expect(room.join("Bad")).rejects.toMatchObject({ code: "invalid_argument" });
    const t = await room.join("table-1");
    expect(await room.join("table-1")).toBe(t);
    expect(t.name).toBe("table-1");
    push("peers", { room: "table-1", peers: [peer(ME, { isMe: true, sameTab: true })] });
    expect((t.peers as () => unknown[])()).toHaveLength(1);
    const onError = vi.fn();
    (t.onPeers as (f: () => void, e: () => void) => void)(() => {}, onError);
    await (t.leave as () => Promise<void>)();
    await (t.leave as () => Promise<void>)();
    expect(onError).not.toHaveBeenCalled();
    expect(calls.filter(c => c.method === "leave")).toEqual([{ method: "leave", args: ["table-1"] }]);
    await expect((t.emit as (x: string) => Promise<void>)("a")).rejects.toMatchObject({ code: "invalid_argument" });
    for (let i = 0; i < MAX_JOINED; i++) await room.join(`r${i}`);
    await expect(room.join("one-more")).rejects.toMatchObject({ code: "limit_reached" });
  });

  it("a terminal error reaches every listener once and every later call", async () => {
    const { room, up, push } = setup();
    up();
    const errors: string[] = [];
    room.onPeers(() => {}, e => errors.push("peers:" + e.code));
    room.on("reaction", () => {}, e => errors.push("on:" + e.code));
    room.onConnection(() => {}, e => errors.push("conn:" + e.code));
    push("peer", { room: null, peer: peer(OTHER) });
    push("error", { room: null, code: "revoked", message: "access changed" });
    push("error", { room: null, code: "revoked", message: "again" });
    expect(errors.sort()).toEqual(["conn:revoked", "on:revoked", "peers:revoked"]);
    await expect(room.emit("a")).rejects.toMatchObject({ code: "revoked" });
    await expect(room.presence({ a: 1 })).rejects.toMatchObject({ code: "revoked" });
    await expect(room.join("x")).rejects.toMatchObject({ code: "revoked" });
    expect(room.connected()).toBe(false);
    expect(room.peers().map(p => p.peer)).toEqual([ME]);
  });

  it("a named room's own error ends that room only", async () => {
    const { room, up, push } = setup();
    up();
    const t = await room.join("table-1");
    const onError = vi.fn();
    (t.on as (x: string, f: () => void, e: () => void) => void)("a", () => {}, onError);
    push("error", { room: "table-1", code: "upstream_error", message: "rejoin failed" });
    expect(onError).toHaveBeenCalledTimes(1);
    await expect((t.emit as (x: string) => Promise<void>)("a")).rejects.toMatchObject({ code: "upstream_error" });
    await expect(room.emit("a")).resolves.toBeUndefined();
    expect(await room.join("table-1")).not.toBe(t);
  });

  it("onConnection fires once with the current state after a microtask, then on each change", async () => {
    const { room, push } = setup();
    const seen: boolean[] = [];
    room.onConnection(c => seen.push(c));
    expect(seen).toEqual([]);
    await Promise.resolve();
    expect(seen).toEqual([false]);
    push("connection", { connected: true });
    push("connection", { connected: true });
    push("connection", { connected: false });
    expect(seen).toEqual([false, true, false]);
    expect(room.connected()).toBe(false);
  });

  it("the part's namespace is frozen and carries exactly the contract's members", () => {
    const f = fakeRpc();
    const ns = roomNamespace(f.rpc as never);
    expect(Object.isFrozen(ns)).toBe(true);
    expect(Object.keys(ns).sort()).toEqual(["canSendToClaudeSession", "connected", "emit", "join", "on", "onConnection", "onPeers", "peers", "presence", "sendToClaudeSession"]);
  });

  it("there is no Claude conversation beside the page", async () => {
    const { room } = setup();
    await expect(room.canSendToClaudeSession()).resolves.toBe("off");
    await expect(room.sendToClaudeSession({ label: "x" })).rejects.toMatchObject({ code: "claude_unavailable" });
  });
});
```

In `web/bridge/test/capabilities.test.ts`, import `ROOM_METHODS` from `../src/caps/room` and add a second `it` to the `describe`:

```ts
  it("room (its part's own list)", () => {
    expect([...ROOM_METHODS].sort()).toEqual(functions(contract("room")));
  });
```

In `web/bridge/test/use.test.ts`, the five `makeUse` calls share one helper (a part capability never reaches `localsFor`):

```ts
import { type UsableName, isPartCapability } from "../src/capabilities";

const locals = (n: UsableName, r: never, c: unknown) =>
  isPartCapability(n) ? Promise.resolve(Object.freeze({ part: n })) : Promise.resolve(localsFor(n, r, c as never, { ctx: commentsContext, clip: () => import("../src/parts/clip") }));
```

(`locals,` in each `makeUse({...})`), the expectation of "aliases self to artifact…" becomes `["artifact", "room", "db"]` (the bridge now asks the shell about `room`; the fake shell refuses it), and a new case pins the dispatch:

```ts
  it("a part capability resolves the namespace its part built, untouched", async () => {
    const rpc = fakeRpc({ room: { topics: {} } });
    const use = makeUse({ framed: true, rpc: rpc as never, locals });
    await expect(use("room")).resolves.toEqual({ part: "room" });
  });
```

In `web/bridge/test/bridge-degraded.test.ts`, change `failParts(["caps", "clip"])` to `failParts(["caps", "clip", "room"])` and add, after the first case:

```ts
  it("a room part that cannot load resolves use(\"room\") null and is reported as room, without loading caps", async () => {
    const before = requests.filter(r => r === "caps").length;
    const use = (window as unknown as { claude: { use(n: string): Promise<unknown> } }).claude.use("room");
    await settle();
    const req = posted.find(m => m.type === "clax:use" && m.name === "room")!;
    send({ type: "clax:use-result", id: req.id, granted: true, config: {} });
    await expect(use).resolves.toBeNull();
    await settle();
    expect(degraded("room")).toHaveLength(1);
    expect(requests.filter(r => r === "caps")).toHaveLength(before);
  });
```

- [ ] **Step 2: Run the bridge tests to verify they fail**

Run: `cd web && npx vitest run bridge/test`
Expected: FAIL (`../src/caps/room` does not exist; `isPartCapability` is not exported; the bridge has no `room` part).

- [ ] **Step 3: Implement the room module and its part**

`web/bridge/src/caps/room.ts`:

```ts
// The `room` capability (contract 0.2.61 room.d.ts), page side, in the
// bridge's lazy `room` part (parts/room.ts). The shell owns
// the WebSocket (web/shell/src/caps/room.ts) and relays the daemon's frames as
// `clax:event`s of ns "room"; this module keeps each room's peers as frozen
// snapshots, applies and coalesces this page's presence, batches onPeers per
// animation frame, checks every argument, and turns terminal errors into one
// onError per listener.
import { CapabilityError, type Rpc } from "../rpc";

export type WirePeer = {
  peer: string; by: string | null; isMe: boolean; sameTab: boolean;
  kind: "viewer" | "agent"; guest: boolean; presence: Record<string, unknown>;
};
export type Peer = Readonly<Omit<WirePeer, "presence"> & { presence: Readonly<Record<string, unknown>>; updatedAt: number }>;
export type PeersChange = Readonly<{ peers: readonly Peer[]; joined: readonly Peer[]; left: readonly Peer[]; updated: readonly Peer[] }>;
type WireMsg = Omit<WirePeer, "presence"> & { topic: string; data?: unknown };
type ErrorInfo = { code: string; message: string };
type OnError = (e: ErrorInfo) => void;

export const TOPIC = /^[a-z][a-z0-9_.-]{0,47}$/;
export const ROOM_NAME = /^[a-z0-9][a-z0-9_.-]{0,47}$/;
const KEY = /^[A-Za-z_][A-Za-z0-9_-]{0,63}$/;
export const MAX_JSON_BYTES = 4096;
export const MAX_DEPTH = 8;
export const MAX_JOINED = 16;
/** Presence is sent at most about 30 times a second. */
export const PRESENCE_INTERVAL_MS = 33;
/** onPeers waits for the next animation frame, or this long where frames do not run. */
export const FLUSH_FALLBACK_MS = 50;
export const JOIN_TIMEOUT_MS = 10_000;

const EMPTY: readonly Peer[] = Object.freeze([]);
const reject = (code: string, message: string) => Promise.reject(new CapabilityError(code, message));
const safe = (fn: () => void) => { try { fn(); } catch (e) { reportError(e); } };

function isPlainObject(v: unknown): v is Record<string, unknown> {
  if (v === null || typeof v !== "object" || Array.isArray(v)) return false;
  const p = Object.getPrototypeOf(v);
  return p === Object.prototype || p === null;
}

function plain(v: unknown, depth: number): boolean {
  if (depth > MAX_DEPTH) return false;
  if (v === null || typeof v === "string" || typeof v === "boolean") return true;
  if (typeof v === "number") return Number.isFinite(v);
  if (Array.isArray(v)) return v.every(x => plain(x, depth + 1));
  return isPlainObject(v) && Object.values(v).every(x => plain(x, depth + 1));
}

/** Why `v` cannot travel as room data (named `what`), or null when it can. */
function jsonProblem(what: string, v: unknown): string | null {
  if (!plain(v, 1)) return `${what} is not plain JSON, or nests deeper than ${MAX_DEPTH} levels`;
  const bytes = new TextEncoder().encode(JSON.stringify(v)).length;
  return bytes > MAX_JSON_BYTES ? `${what} is ${bytes} bytes of JSON; the limit is ${MAX_JSON_BYTES}` : null;
}

function freezePeer(w: WirePeer, updatedAt: number): Peer {
  const presence = Object.freeze(JSON.parse(JSON.stringify(w.presence ?? {})) as Record<string, unknown>);
  return Object.freeze({ peer: w.peer, by: w.by ?? null, isMe: !!w.isMe, sameTab: !!w.sameTab, kind: w.kind === "agent" ? "agent" : "viewer", guest: !!w.guest, presence, updatedAt });
}

type PeerSub = { fn: (c: PeersChange) => void; onError?: OnError; primed: boolean };
type TopicSub = { topic: string; fn: (m: Readonly<WireMsg>) => void; onError?: OnError };
type ConnSub = { fn: (c: boolean) => void; onError?: OnError };

/** Shared by the lobby and every named room. */
interface Runtime {
  rpc: Pick<Rpc, "call" | "on">;
  connected: boolean;
  me: string | null;
  reportDrop(): void;
}

/** One room (the lobby, or a named room) as this page sees it. */
class Scope {
  private readonly peers = new Map<string, Peer>();
  private snapshot: readonly Peer[] = EMPTY;
  private answered = false;
  private mine: Record<string, unknown> = {};
  private readonly topicSubs = new Set<TopicSub>();
  private readonly peerSubs = new Set<PeerSub>();
  readonly connSubs = new Set<ConnSub>();
  private readonly changes = new Map<string, "joined" | "updated" | "left">();
  private readonly gone = new Map<string, Peer>();
  private flushQueued = false;
  /** The peers changed since `snapshot` was built. */
  private dirty = false;
  private sendTimer: ReturnType<typeof setTimeout> | null = null;
  private lastSent = 0;
  dead: ErrorInfo | null = null;

  constructor(readonly name: string | null, private readonly rt: Runtime) {}

  // ---- frames from the shell ----

  onPeersFrame(list: WirePeer[]): void {
    const seen = new Set<string>();
    for (const w of list) { seen.add(w.peer); this.upsert(w); }
    for (const label of [...this.peers.keys()]) if (!seen.has(label)) this.remove(label);
    this.answered = true;
    this.commit();
  }

  onPeerFrame(w: WirePeer): void { this.upsert(w); this.commit(); }

  onLeftFrame(label: string): void { this.remove(label); this.commit(); }

  onMsgFrame(m: WireMsg): void {
    if (this.dead) return;
    const msg = Object.freeze({ peer: m.peer, by: m.by ?? null, isMe: !!m.isMe, sameTab: !!m.sameTab, kind: m.kind === "agent" ? "agent" : "viewer", guest: !!m.guest, topic: m.topic, ...(m.data === undefined ? {} : { data: m.data }) }) as Readonly<WireMsg>;
    for (const sub of [...this.topicSubs]) if (sub.topic === m.topic && this.topicSubs.has(sub)) safe(() => sub.fn(msg));
  }

  private upsert(w: WirePeer): void {
    if (w.sameTab) w = { ...w, presence: this.mine };
    const prev = this.peers.get(w.peer);
    const samePresence = !!prev && JSON.stringify(prev.presence) === JSON.stringify(w.presence ?? {});
    if (prev && samePresence && prev.by === (w.by ?? null) && prev.isMe === !!w.isMe) return;
    this.peers.set(w.peer, freezePeer(w, prev && samePresence ? prev.updatedAt : Date.now()));
    this.dirty = true;
    this.note(w.peer, prev ? "updated" : "joined");
  }

  private remove(label: string): void {
    const prev = this.peers.get(label);
    if (!prev) return;
    this.peers.delete(label);
    this.dirty = true;
    this.note(label, "left", prev);
  }

  private note(label: string, kind: "joined" | "updated" | "left", prev?: Peer): void {
    const before = this.changes.get(label);
    if (kind === "left") {
      if (before === "joined") { this.changes.delete(label); return; }
      this.changes.set(label, "left");
      if (prev && !this.gone.has(label)) this.gone.set(label, prev);
      return;
    }
    if (before === "left") { this.changes.set(label, "updated"); this.gone.delete(label); return; }
    if (before === "joined") return;
    this.changes.set(label, kind);
  }

  private commit(): void {
    if (this.dirty) {
      this.snapshot = Object.freeze([...this.peers.values()]);
      this.dirty = false;
    }
    this.schedule();
  }

  private schedule(): void {
    if (this.flushQueued || !this.peerSubs.size) return;
    this.flushQueued = true;
    const run = () => { if (!this.flushQueued) return; this.flushQueued = false; this.flush(); };
    if (typeof requestAnimationFrame === "function") requestAnimationFrame(run);
    setTimeout(run, FLUSH_FALLBACK_MS);
  }

  private flush(): void {
    const joined: Peer[] = [], updated: Peer[] = [], left: Peer[] = [];
    for (const [label, kind] of this.changes) {
      if (kind === "left") { const p = this.gone.get(label); if (p) left.push(p); continue; }
      const p = this.peers.get(label);
      if (p) (kind === "joined" ? joined : updated).push(p);
    }
    this.changes.clear();
    this.gone.clear();
    const none: readonly Peer[] = EMPTY;
    for (const sub of [...this.peerSubs]) {
      if (!this.peerSubs.has(sub) || this.dead) continue;
      if (!sub.primed) {
        if (!this.answered) continue;
        sub.primed = true;
        safe(() => sub.fn(Object.freeze({ peers: this.snapshot, joined: this.snapshot, left: none, updated: none })));
      } else if (joined.length || updated.length || left.length) {
        safe(() => sub.fn(Object.freeze({ peers: this.snapshot, joined: Object.freeze(joined), left: Object.freeze(left), updated: Object.freeze(updated) })));
      }
    }
  }

  // ---- the page's calls ----

  peersNow(): readonly Peer[] {
    if (!this.dead) return this.snapshot;
    return Object.freeze(this.snapshot.filter(p => p.sameTab));
  }

  onPeers(fn: unknown, onError?: OnError): () => void {
    if (typeof fn !== "function") throw new TypeError("onPeers takes a function");
    if (this.dead) { const d = this.dead; queueMicrotask(() => onError?.(d)); return () => {}; }
    const sub: PeerSub = { fn: fn as PeerSub["fn"], onError, primed: false };
    this.peerSubs.add(sub);
    this.schedule();
    return () => { this.peerSubs.delete(sub); };
  }

  on(topic: unknown, fn: unknown, onError?: OnError): () => void {
    if (typeof fn !== "function") throw new TypeError("on takes a handler function");
    if (typeof topic !== "string" || !TOPIC.test(topic)) {
      const e = { code: "invalid_argument", message: `'${String(topic)}' is not a topic (^[a-z][a-z0-9_.-]{0,47}$)` };
      queueMicrotask(() => onError?.(e));
      return () => {};
    }
    if (this.dead) { const d = this.dead; queueMicrotask(() => onError?.(d)); return () => {}; }
    const sub: TopicSub = { topic, fn: fn as TopicSub["fn"], onError };
    this.topicSubs.add(sub);
    return () => { this.topicSubs.delete(sub); };
  }

  onConnection(fn: unknown, onError?: OnError): () => void {
    if (typeof fn !== "function") throw new TypeError("onConnection takes a function");
    if (this.dead) { const d = this.dead; queueMicrotask(() => onError?.(d)); return () => {}; }
    const sub: ConnSub = { fn: fn as ConnSub["fn"], onError };
    this.connSubs.add(sub);
    queueMicrotask(() => { if (this.connSubs.has(sub)) safe(() => sub.fn(this.rt.connected && !this.dead)); });
    return () => { this.connSubs.delete(sub); };
  }

  connectionChanged(c: boolean): void {
    for (const sub of [...this.connSubs]) safe(() => sub.fn(c));
  }

  presence(patch: unknown): Promise<void> {
    if (this.dead) return reject(this.dead.code, this.dead.message);
    if (!isPlainObject(patch)) return reject("invalid_argument", "presence takes a plain object of fields");
    const merged: Record<string, unknown> = { ...this.mine };
    for (const [k, v] of Object.entries(patch)) {
      if (!KEY.test(k) || k in Object.prototype || k === "prototype") return reject("invalid_argument", `presence key '${k}' is not an identifier`);
      if (v === null) delete merged[k]; else merged[k] = v;
    }
    const problem = jsonProblem("presence", merged);
    if (problem) return reject("invalid_argument", problem);
    this.mine = JSON.parse(JSON.stringify(merged)) as Record<string, unknown>;
    const me = this.rt.me && this.peers.get(this.rt.me);
    if (me) {
      this.peers.set(me.peer, freezePeer({ ...me, presence: this.mine }, Date.now()));
      this.dirty = true;
      this.note(me.peer, "updated");
      this.commit();
    }
    this.queueSend();
    return Promise.resolve();
  }

  private queueSend(): void {
    if (this.sendTimer) return;
    const wait = Math.max(0, this.lastSent + PRESENCE_INTERVAL_MS - Date.now());
    this.sendTimer = setTimeout(() => {
      this.sendTimer = null;
      if (this.dead) return;
      this.lastSent = Date.now();
      void this.rt.rpc.call("room", "presence", [this.name, this.mine]).catch(() => {});
    }, wait);
  }

  emit(topic: unknown, data?: unknown): Promise<void> {
    if (this.dead) return reject(this.dead.code, this.dead.message);
    if (typeof topic !== "string" || !TOPIC.test(topic)) return reject("invalid_argument", `'${String(topic)}' is not a topic (^[a-z][a-z0-9_.-]{0,47}$)`);
    if (data !== undefined) { const p = jsonProblem("data", data); if (p) return reject("invalid_argument", p); }
    if (!this.rt.connected) return Promise.resolve();
    const args = data === undefined ? [this.name, topic] : [this.name, topic, data];
    return this.rt.rpc.call("room", "emit", args).then(r => {
      if ((r as { dropped?: boolean } | null)?.dropped) this.rt.reportDrop();
    });
  }

  /** Ends this scope: listeners hear `e` once (unless `silent`), calls reject with it. */
  die(e: ErrorInfo, silent = false): void {
    if (this.dead) return;
    this.dead = e;
    if (this.sendTimer) { clearTimeout(this.sendTimer); this.sendTimer = null; }
    const subs = [...this.topicSubs, ...this.peerSubs, ...this.connSubs];
    this.topicSubs.clear(); this.peerSubs.clear(); this.connSubs.clear();
    if (!silent) for (const s of subs) if (s.onError) safe(() => s.onError!(e));
  }
}

type Named = { scope: Scope; api: Readonly<Record<string, unknown>>; ready: Promise<Readonly<Record<string, unknown>>> };

/** The `room` namespace members; the shell is asked to connect at once. */
export function makeRoom(rpc: Pick<Rpc, "call" | "on">): Record<string, (...args: never[]) => unknown> {
  let dropReported = false;
  const rt: Runtime = {
    rpc, connected: false, me: null,
    reportDrop() {
      if (dropReported) return;
      dropReported = true;
      reportError(new Error("room: emits past about 40 a second were dropped"));
    },
  };
  const lobby = new Scope(null, rt);
  const named = new Map<string, Named>();
  const scope = (room: unknown): Scope | undefined => (room === null || room === undefined ? lobby : named.get(String(room))?.scope);

  rpc.on("room", "connection", d => {
    const c = !!(d as { connected: boolean }).connected;
    if (c === rt.connected || lobby.dead) return;
    rt.connected = c;
    lobby.connectionChanged(c);
    for (const n of named.values()) if (!n.scope.dead) n.scope.connectionChanged(c);
  });
  rpc.on("room", "welcome", d => { rt.me = String((d as { peer: string }).peer); });
  rpc.on("room", "peers", d => { const f = d as { room: string | null; peers: WirePeer[] }; scope(f.room)?.onPeersFrame(f.peers); });
  rpc.on("room", "peer", d => { const f = d as { room: string | null; peer: WirePeer }; scope(f.room)?.onPeerFrame(f.peer); });
  rpc.on("room", "left", d => { const f = d as { room: string | null; peer: string }; scope(f.room)?.onLeftFrame(f.peer); });
  rpc.on("room", "msg", d => { const f = d as { room: string | null; msg: WireMsg }; scope(f.room)?.onMsgFrame(f.msg); });
  rpc.on("room", "error", d => {
    const f = d as { room: string | null; code: string; message: string };
    const e = { code: String(f.code), message: String(f.message) };
    if (f.room === null) {
      rt.connected = false;
      lobby.die(e);
      for (const n of named.values()) n.scope.die(e);
      named.clear();
      return;
    }
    const n = named.get(f.room);
    if (n) { named.delete(f.room); n.scope.die(e); }
  });
  void rpc.call("room", "connect", []).catch(() => {});

  function makeNamed(s: Scope, name: string): Readonly<Record<string, unknown>> {
    let left = false;
    return Object.freeze({
      name,
      emit: (topic: unknown, data?: unknown) => s.emit(topic, data),
      on: (topic: unknown, fn: unknown, onError?: OnError) => s.on(topic, fn, onError),
      presence: (patch: unknown) => s.presence(patch),
      peers: () => s.peersNow(),
      onPeers: (fn: unknown, onError?: OnError) => s.onPeers(fn, onError),
      connected: () => rt.connected && !s.dead,
      onConnection: (fn: unknown, onError?: OnError) => s.onConnection(fn, onError),
      leave: () => {
        if (left) return Promise.resolve();
        left = true;
        if (named.get(name)?.scope === s) named.delete(name);
        s.die({ code: "invalid_argument", message: `this page left room '${name}'` }, true);
        void rpc.call("room", "leave", [name]).catch(() => {});
        return Promise.resolve();
      },
    });
  }

  function join(name: unknown): Promise<Readonly<Record<string, unknown>>> {
    if (lobby.dead) return reject(lobby.dead.code, lobby.dead.message);
    if (typeof name !== "string" || !ROOM_NAME.test(name)) return reject("invalid_argument", `'${String(name)}' is not a room name (^[a-z0-9][a-z0-9_.-]{0,47}$)`);
    const have = named.get(name);
    if (have) return have.ready;
    if (named.size >= MAX_JOINED) return reject("limit_reached", `a page may be in at most ${MAX_JOINED} named rooms; leave one first`);
    const s = new Scope(name, rt);
    const api = makeNamed(s, name);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<never>((_, no) => {
      timer = setTimeout(() => no(new CapabilityError("upstream_error", `joining '${name}' got no answer within 10 s`)), JOIN_TIMEOUT_MS);
    });
    const answered = Promise.race([rpc.call("room", "join", [name]), timeout]).finally(() => clearTimeout(timer));
    const ready = answered.then(() => api, e => {
      if (named.get(name)?.scope === s) named.delete(name);
      s.die({ code: "upstream_error", message: "join failed" }, true);
      throw e;
    });
    named.set(name, { scope: s, api, ready });
    return ready;
  }

  return {
    emit: (topic: unknown, data?: unknown) => lobby.emit(topic, data),
    on: (topic: unknown, fn: unknown, onError?: OnError) => lobby.on(topic, fn, onError),
    presence: (patch: unknown) => lobby.presence(patch),
    peers: () => lobby.peersNow(),
    onPeers: (fn: unknown, onError?: OnError) => lobby.onPeers(fn, onError),
    connected: () => rt.connected && !lobby.dead,
    onConnection: (fn: unknown, onError?: OnError) => lobby.onConnection(fn, onError),
    join,
    canSendToClaudeSession: () => Promise.resolve("off"),
    sendToClaudeSession: () => reject("claude_unavailable", "there is no Claude conversation beside this page in Clax"),
  } as unknown as Record<string, (...args: never[]) => unknown>;
}

/** The members of the `room` namespace (checked against room.d.ts in
 * bridge/test/capabilities.test.ts). */
export const ROOM_METHODS = ["emit", "on", "presence", "peers", "onPeers", "sendToClaudeSession", "canSendToClaudeSession", "join", "connected", "onConnection"] as const;

/** The frozen `room` namespace `claude.use("room")` resolves. */
export function roomNamespace(rpc: Pick<Rpc, "call" | "on">): Readonly<Record<string, unknown>> {
  const m = makeRoom(rpc);
  return Object.freeze(Object.fromEntries(ROOM_METHODS.map(k => [k, m[k]])));
}
```

`web/bridge/src/parts/room.ts`:

```ts
// The `room` capability's page side: loaded on the first claude.use("room")
// the shell grants, never with the other capabilities' members (parts/caps.ts).
export { ROOM_METHODS, roomNamespace } from "../caps/room";
```

- [ ] **Step 4: Dispatch part capabilities in the eager bridge**

`web/bridge/src/capabilities.ts`, below `isCapabilityName` (which it replaces):

```ts
/** Capabilities whose page side is a lazy part of its own (parts/<name>.ts):
 * the part builds the whole frozen namespace, and its member list lives there. */
export const PART_CAPABILITIES = ["room", "sample"] as const;
export type PartCapabilityName = (typeof PART_CAPABILITIES)[number];
export type UsableName = CapabilityName | PartCapabilityName;

export function isPartCapability(name: string): name is PartCapabilityName {
  return (PART_CAPABILITIES as readonly string[]).includes(name);
}

export function isCapabilityName(name: string): name is UsableName {
  return Object.prototype.hasOwnProperty.call(CAPABILITY_METHODS, name) || isPartCapability(name);
}
```

`web/bridge/src/use.ts`: the header comment's last sentence becomes "The page-side members come from `opts.locals` (a lazy part in the bridge: `caps` for most capabilities, which returns members that `buildNamespace` completes, or the capability's own part, which returns the finished namespace); one that cannot load resolves null, as a refused capability does." The import gains `isPartCapability, type UsableName`, the option becomes `locals: (name: UsableName, rpc: Rpc, config: unknown) => Promise<unknown>`, and the resolve line becomes:

```ts
      const local = await opts.locals(key, opts.rpc, grant.config);
      return isPartCapability(key) ? local : buildNamespace(key, opts.rpc, local as Local);
```

`web/bridge/src/parts/types.ts`:

```ts
export type CommentPart = typeof import("./comment");
export type ClipPart = typeof import("./clip");
export type CapsPart = typeof import("./caps");
export type RoomPart = typeof import("./room");
/** The parts' loaders. `attempt` (0 first) numbers a retry after a failure,
 * which a loader by URL loads afresh. */
export type Parts = {
  comment(attempt?: number): Promise<CommentPart>;
  clip(attempt?: number): Promise<ClipPart>;
  caps(attempt?: number): Promise<CapsPart>;
  room(attempt?: number): Promise<RoomPart>;
};
```

`web/bridge/src/parts-url.ts` builds every part's URL the same way, so a part costs the eager bridge only its name (the comment block above `import` stays):

```ts
import type { Parts } from "./parts/types";

declare const __CLAX_PARTS__: Record<keyof Parts, string>;

/** The parts' loaders; `bridgeSrc` is the bridge script's own URL. */
export function loadParts(bridgeSrc: string): Parts {
  const href: Partial<Record<keyof Parts, string>> = {};
  try {
    const base = new URL("/_clax/bridge/", bridgeSrc);
    for (const name of Object.keys(__CLAX_PARTS__) as (keyof Parts)[]) href[name] = new URL(__CLAX_PARTS__[name], base).href;
  } catch { /* no URL to load from: every part fails */ }
  // A browser keeps a failed module load for its URL, so a retry asks for
  // the same file under a query naming the attempt.
  const load = (name: keyof Parts) => (attempt = 0) => {
    const u = href[name];
    return u ? import(/* @vite-ignore */ attempt ? `${u}?retry=${attempt}` : u) : Promise.reject(new Error(`no URL for the ${name} part`));
  };
  return { comment: load("comment"), clip: load("clip"), caps: load("caps"), room: load("room") } as Parts;
}
```

`web/bridge/src/parts-static.ts`: add `room: loader<RoomPart>("room", () => import("./parts/room")),` (and `RoomPart` to its type import).

`web/bridge/src/bridge.ts`: in the header comment, "capability members on the first claude.use() the shell grants" becomes "a capability's members on the first claude.use() of it the shell grants (`room` and `sample` each in a part of its own, the rest in `caps`)". `parts` gains `room: onNeed("room", loaders.room)`, and `locals` dispatches by name:

```ts
    locals: (name, r, config) => name === "room"
      ? parts.room().then(p => p.roomNamespace(r))
      : parts.caps().then(c => c.localsFor(name as CapabilityName, r, config, { ctx: commentsContext, clip: clips })),
```

(`import type { CapabilityName } from "./capabilities";`; Task 7 adds the `sample` arm, which narrows `name` so the cast goes.)

`web/bridge/src/protocol.ts`: `clax:degraded`'s `part` is `"comment" | "clip" | "caps" | "room" | "sample"`.

`web/scripts/build-parts.mjs`: `const PARTS = ["comment", "clip", "caps", "room"];` (its header comment names `bridge/src/parts/{comment,clip,caps,room}.ts`). `web/vite.bridge.config.ts`: the required names are `["comment", "clip", "caps", "room"]`. `web/scripts/bundle-size.mjs`: `partKeys` gains `room: "partRoom"`, and the log line names it.

`web/shell/src/failure.ts` (the notice's words; open question Q7, **align with Echo at merge**):

```ts
export const PART_FAILED: Record<"comment" | "clip" | "caps" | "room" | "sample", string> = {
  comment: "Comment mode could not load in this page",
  clip: "Screenshots could not load in this page",
  caps: "This page's capabilities could not load",
  room: "This page's live room could not load",
  sample: "This page's Claude calls could not load",
};
```

- [ ] **Step 5: Run the bridge and shell unit tests to verify they pass**

Run: `cd web && npm test -- --reporter=dot && npm run typecheck && npm run lint`
Expected: PASS.

- [ ] **Step 6: Measure, and add the part's budget**

Run: `cd web && npm run build && node scripts/bundle-size.mjs`
Expected: it fails only because `bundle-budget.json` has no `partRoom` budget. Add `"partRoom": <floor(measured × 1.1)>` by hand (the value `--record` would write; do not run `--record`, which would also lower every other budget). Run `node scripts/bundle-size.mjs` again.
Expected: PASS, and in its output `eager bridge` is at most 5491 (its budget), `caps` is unchanged from before this task, and `artifact` is unchanged. If the eager bridge is over its budget, the dispatch is too large: shorten it (the `PART_CAPABILITIES` list and the two arms are all it should add), never raise the budget.

- [ ] **Step 7: The parts e2e**

In `web/e2e/bridge-parts.spec.ts`, beside its existing check of which parts a page loads, add a case: a page declaring `{"db": {}}` that calls `claude.use("db")` loads the `caps` part and never requests a URL under `/_clax/bridge/room`, and a page declaring `{"room": {}}` that calls `claude.use("room")` loads `room` and never `caps` (record requests with `page.on("request")`; both frame modes).

Run: `cd web && npx playwright test bridge-parts.spec.ts bridge-comment.spec.ts`
Expected: PASS in both modes.

- [ ] **Step 8: Stage**

Stage the change (`git add web/bridge web/scripts web/vite.bridge.config.ts web/perf/bundle-budget.json web/shell/src/failure.ts web/e2e/bridge-parts.spec.ts`); do not commit. The controller commits it with the message:

```text
Load room's page side as a lazy bridge part of its own, and give part capabilities their own namespaces
```

---

### Task 6: `room` in the shell: a lazy handler, leaving with the document, and the browser tests

The shell end of the room relay: one WebSocket per open document, owned by the shell in both frame modes. The handler is carried over from the earlier draft of this plan, renamed, with two additions: it reconnects under the same label when the viewer's name changes (scan F6), and it closes when the frame's document leaves (scan B5, the ruling above). It loads as a separate chunk on the page's first room call, so the artifact entry does not grow (scan F14). No shell UI shows anything about rooms (open question Q4).

**Files:**
- Create: `web/shell/src/caps/lazy.ts`, `web/shell/src/caps/room.ts`
- Modify: `web/shell/src/caps/host.ts` (`Handler.leave`, `CapabilityHost.leave`), `web/shell/src/caps/registry.ts` (`room`), `web/shell/src/caps/availability.ts` (`room` when declared; `CAPABILITIES` gains `room`), `web/shell/src/view/artifact-controller.ts` (`leaveDocument` wherever the gate closes)
- Create: `web/e2e/pages/room.html`, `web/e2e/room.spec.ts`
- Modify: `web/e2e/contract.spec.ts` (a case for `room.html`)
- Test: `web/shell/src/caps/lazy.test.ts`, `web/shell/src/caps/room.test.ts` (new); `web/shell/src/caps/host.test.ts`, `web/shell/src/caps/grants.test.ts`, `web/shell/src/view/artifact-controller.test.ts` (updated)

**Interfaces:**
- Consumes: `HandlerFactory`, `Handler`, `CapEnv.{aid, token, post}`, `CapError`, `REGISTRY`, `isAvailable`, `CAPABILITIES`, `onViewer` (`threads.ts`), `FrameGate`; Task 1's socket and frames; Task 5's `room` part.
- Produces:
  - `lazyHandler(load: () => Promise<HandlerFactory>): HandlerFactory`.
  - `Handler.leave?(): void`: the frame's document left (a `bye`, a load without a hello, a hello the shell does not welcome, a navigation the shell started); close what serves that document live. `CapabilityHost.leave(): void` calls it on every handler.
  - `makeRoomHandler(open?: OpenSocket, timer?: Timer, viewers?: Viewers): HandlerFactory`, `roomHandler`, `peerLabel(): string`, `RECONNECT_MS = [1000, 2000, 5000]`, `ACK_TIMEOUT_MS = 10_000`, types `SocketLike`, `OpenSocket`, `Timer`, `Viewers`; `REGISTRY.room`.
  - The Room relay of the Shared contract.

- [ ] **Step 1: Write the failing shell tests**

`web/shell/src/caps/lazy.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import type { CapEnv, Handler } from "./host";
import { lazyHandler } from "./lazy";

const env = {} as CapEnv;
const grants = {} as never;

describe("lazyHandler", () => {
  it("loads the handler on the first call only, once, and forwards the lifecycle to it", async () => {
    const inner: Handler = { call: vi.fn(async () => 7), reset: vi.fn(), leave: vi.fn(), dispose: vi.fn() };
    const load = vi.fn(async () => () => inner);
    const h = lazyHandler(load)(env, grants);
    h.leave!();
    expect(load).not.toHaveBeenCalled();
    await expect(h.call("x", [])).resolves.toBe(7);
    await h.call("y", []);
    expect(load).toHaveBeenCalledTimes(1);
    h.reset!();
    h.leave!();
    h.dispose!();
    expect([inner.reset, inner.leave, inner.dispose].map(f => (f as ReturnType<typeof vi.fn>).mock.calls.length)).toEqual([1, 1, 1]);
  });

  it("a chunk that cannot load rejects capability_disabled, and the next call tries again", async () => {
    const load = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue(() => ({ call: async () => "ok" }));
    const h = lazyHandler(load)(env, grants);
    await expect(h.call("x", [])).rejects.toMatchObject({ code: "capability_disabled" });
    await expect(h.call("x", [])).resolves.toBe("ok");
  });

  it("after dispose, a chunk that arrives late makes no handler", async () => {
    let arrive!: (f: () => Handler) => void;
    const make = vi.fn(() => ({ call: async () => 1 }));
    const h = lazyHandler(() => new Promise(r => { arrive = r; }))(env, grants);
    const p = h.call("x", []);
    h.dispose!();
    arrive(make);
    await expect(p).rejects.toMatchObject({ code: "capability_disabled" });
    expect(make).not.toHaveBeenCalled();
  });
});
```

`web/shell/src/caps/room.test.ts`:

```ts
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import type { CapEnv } from "./host";
import { ACK_TIMEOUT_MS, RECONNECT_MS, makeRoomHandler, type SocketLike } from "./room";

class FakeSocket implements SocketLike {
  static all: FakeSocket[] = [];
  readyState = 0;
  sent: unknown[] = [];
  onopen: ((e: unknown) => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  constructor(readonly url: string) { FakeSocket.all.push(this); }
  send(data: string) { this.sent.push(JSON.parse(data)); }
  close(code = 1000, reason = "") { this.readyState = 3; this.onclose?.({ code, reason }); }
  open() { this.readyState = 1; this.onopen?.({}); }
  frame(v: unknown) { this.onmessage?.({ data: JSON.stringify(v) }); }
  drop(code = 1006, reason = "") { this.readyState = 3; this.onclose?.({ code, reason }); }
}

type Viewers = (fn: (v: unknown) => void) => () => void;

function setup(token: string | null = null, viewers: Viewers = () => () => {}) {
  FakeSocket.all = [];
  const posted: ShellToBridge[] = [];
  const env = { aid: "7q3k9mzx2b4t", token, post: (m: ShellToBridge) => posted.push(m) } as unknown as CapEnv;
  const handler = makeRoomHandler(url => new FakeSocket(url), undefined, viewers)(env, {} as never);
  const events = () => posted.filter(m => m.type === "clax:event").map(m => m as { topic: string; data: unknown });
  return { handler, posted, events, sock: () => FakeSocket.all.at(-1)! };
}

describe("room handler", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("opens one socket per document with a 16-character label and relays frames", async () => {
    const { handler, events, sock } = setup();
    await handler.call("connect", []);
    await handler.call("connect", []);
    expect(FakeSocket.all).toHaveLength(1);
    expect(sock().url).toMatch(/^ws:\/\/[^/]+\/api\/artifacts\/7q3k9mzx2b4t\/room\?peer=[0-9a-v]{16}$/);
    sock().open();
    sock().frame({ t: "welcome", peer: "p" });
    sock().frame({ t: "peers", room: null, peers: [] });
    sock().frame({ t: "msg", room: null, msg: { topic: "a" } });
    expect(events().map(e => e.topic)).toEqual(["connection", "welcome", "peers", "msg"]);
    expect(events()[0].data).toEqual({ connected: true });
  });

  it("the owner shell appends its token; a shell without one does not", async () => {
    const owner = setup("tok/en+1");
    await owner.handler.call("connect", []);
    expect(owner.sock().url).toMatch(/\?peer=[0-9a-v]{16}&token=tok%2Fen%2B1$/);
    const lan = setup();
    await lan.handler.call("connect", []);
    expect(lan.sock().url).not.toContain("token=");
  });

  it("emit waits for the ack, passes a nack's code through, and times out as upstream_error", async () => {
    const { handler, sock } = setup();
    await handler.call("connect", []);
    sock().open();
    const ok = handler.call("emit", [null, "reaction", { k: 1 }]);
    const sent = sock().sent.at(-1) as { id: number };
    expect(sent).toEqual({ t: "emit", id: sent.id, room: null, topic: "reaction", data: { k: 1 } });
    sock().frame({ t: "ack", id: sent.id, dropped: true });
    await expect(ok).resolves.toEqual({ dropped: true });
    const bare = handler.call("emit", [null, "bare"]);
    expect(sock().sent.at(-1)).not.toHaveProperty("data");
    sock().frame({ t: "nack", id: (sock().sent.at(-1) as { id: number }).id, code: "not_permitted", message: "admin only" });
    await expect(bare).rejects.toMatchObject({ code: "not_permitted" });
    const slow = handler.call("emit", [null, "x"]);
    const settled = expect(slow).rejects.toMatchObject({ code: "upstream_error" });
    await vi.advanceTimersByTimeAsync(ACK_TIMEOUT_MS);
    await settled;
  });

  it("an emit while the socket is down resolves without sending", async () => {
    const { handler, sock } = setup();
    await handler.call("connect", []);
    await expect(handler.call("emit", [null, "x"])).resolves.toEqual({ dropped: false });
    expect(sock().sent).toEqual([]);
  });

  it("reconnects with backoff under the same label, re-sending presence and re-joining rooms", async () => {
    const { handler, events, sock } = setup();
    await handler.call("connect", []);
    const first = sock();
    first.open();
    await handler.call("presence", [null, { pick: "B" }]);
    const joining = handler.call("join", ["table-1"]);
    first.frame({ t: "ack", id: (first.sent.at(-1) as { id: number }).id });
    await joining;
    first.drop();
    expect(events().at(-1)).toEqual({ type: "clax:event", ns: "room", topic: "connection", data: { connected: false } });
    await vi.advanceTimersByTimeAsync(RECONNECT_MS[0] - 1);
    expect(FakeSocket.all).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    const second = sock();
    expect(second).not.toBe(first);
    expect(second.url).toBe(first.url);
    second.open();
    expect(second.sent).toEqual([
      { t: "presence", room: null, state: { pick: "B" } },
      { t: "join", id: expect.any(Number), room: "table-1" },
    ]);
  });

  it("a failed re-join ends that room for the page", async () => {
    const { handler, events, sock } = setup();
    await handler.call("connect", []);
    sock().open();
    const joining = handler.call("join", ["table-1"]);
    sock().frame({ t: "ack", id: (sock().sent.at(-1) as { id: number }).id });
    await joining;
    sock().drop();
    await vi.advanceTimersByTimeAsync(RECONNECT_MS[0]);
    sock().open();
    sock().frame({ t: "nack", id: (sock().sent.at(-1) as { id: number }).id, code: "limit_reached", message: "full" });
    await vi.advanceTimersByTimeAsync(0);
    expect(events().at(-1)!.data).toEqual({ room: "table-1", code: "limit_reached", message: "full" });
  });

  it("4403 is terminal: an error for the page and no reconnect; 4409 stops quietly", async () => {
    const { handler, events, sock } = setup();
    await handler.call("connect", []);
    sock().open();
    sock().drop(4403, "revoked");
    expect(events().at(-1)!.data).toEqual({ room: null, code: "revoked", message: expect.any(String) });
    await vi.advanceTimersByTimeAsync(10_000);
    expect(FakeSocket.all).toHaveLength(1);

    const again = setup();
    await again.handler.call("connect", []);
    again.sock().drop(4403, "not_granted");
    expect(again.events().at(-1)!.data).toMatchObject({ room: null, code: "not_granted" });

    const replaced = setup();
    await replaced.handler.call("connect", []);
    replaced.sock().open();
    replaced.sock().drop(4409, "replaced");
    await vi.advanceTimersByTimeAsync(10_000);
    expect(FakeSocket.all).toHaveLength(1);
  });

  it.each(["reset", "leave"] as const)("%s closes the socket and the next document gets a new label", async which => {
    const { handler, sock } = setup();
    await handler.call("connect", []);
    const first = sock();
    first.open();
    handler[which]!();
    expect(first.readyState).toBe(3);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(FakeSocket.all).toHaveLength(1);
    await handler.call("connect", []);
    expect(sock()).not.toBe(first);
    expect(sock().url).not.toBe(first.url);
  });

  it("a rename reconnects under the same label without telling the page it went offline", async () => {
    let rename = () => {};
    const off = vi.fn();
    const { handler, events, sock } = setup(null, fn => { rename = () => fn({}); return off; });
    await handler.call("connect", []);
    const first = sock();
    first.open();
    await handler.call("presence", [null, { pick: "B" }]);
    rename();
    const second = sock();
    expect(second).not.toBe(first);
    expect(second.url).toBe(first.url);
    second.open();
    expect(first.readyState).toBe(3);
    expect(second.sent).toEqual([{ t: "presence", room: null, state: { pick: "B" } }]);
    expect(events().filter(e => e.topic === "connection").map(e => e.data)).toEqual([{ connected: true }, { connected: true }]);
    handler.dispose!();
    expect(off).toHaveBeenCalledTimes(1);
  });
});
```

In `web/shell/src/caps/host.test.ts`, add:

```ts
  it("tells every handler the frame's document left, and nothing after dispose", async () => {
    const { e } = env();
    const leave = vi.fn();
    const host = new CapabilityHost(Promise.resolve(e), { db: () => ({ call: async () => 1, leave }) }, null);
    await host.handle({ type: "clax:call", id: "1", ns: "db", method: "get", args: [] });
    host.leave();
    expect(leave).toHaveBeenCalledTimes(1);
    host.dispose();
    host.leave();
    expect(leave).toHaveBeenCalledTimes(1);
  });

  it("grants room to any view of an artifact that declares it", async () => {
    const { e, posted } = env({ declared: { room: {} }, token: null });
    await new CapabilityHost(Promise.resolve(e), REGISTRY, null).handle({ type: "clax:use", id: "u", name: "room" });
    expect(posted[0]).toMatchObject({ granted: true, config: {} });
  });
```

In `web/shell/src/caps/grants.test.ts`, add a case: with `{ room: {} }` declared, `state("room")` is `"granted"` (no consent) and `all()` lists it.

In `web/shell/src/view/artifact-controller.test.ts`, add:

```ts
  it("tells the capability host the page left at a bye, at a load with no hello since, and at a hello it does not welcome", async () => {
    const { ctl, frame } = await started();
    const { CapabilityHost } = await import("../caps/host");
    const leave = vi.spyOn(CapabilityHost.prototype, "leave");
    frame.contentWindow!.postMessage = (() => {}) as Window["postMessage"];
    hello(frame.contentWindow!);
    fromFrame(frame.contentWindow!, { type: "clax:bye" });
    expect(leave).toHaveBeenCalledTimes(1);
    hello(frame.contentWindow!);
    ctl.frameLoaded(); // the hello came since the previous load: the page stays
    expect(leave).toHaveBeenCalledTimes(1);
    ctl.frameLoaded(); // no hello since the previous load: that document left
    expect(leave).toHaveBeenCalledTimes(2);
    hello(frame.contentWindow!);
    hello(frame.contentWindow!, 1); // a stale document now holds the frame
    expect(leave).toHaveBeenCalledTimes(3);
    ctl.dispose();
  });
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && npx vitest run shell/src`
Expected: FAIL (`./lazy`, `./room` do not exist; `CapabilityHost.leave` is not a function; `room` is not granted).

- [ ] **Step 3: Implement the lazy handler, `leave`, and the room handler**

`web/shell/src/caps/lazy.ts`:

```ts
// A capability handler whose code is a chunk of its own, loaded on the page's
// first call to the capability: the artifact entry never carries it (spec §8,
// time to usable). A chunk that cannot load rejects that call
// `capability_disabled`, and the next call tries again.
import { CapError } from "./errors";
import type { Handler, HandlerFactory } from "./host";

export function lazyHandler(load: () => Promise<HandlerFactory>): HandlerFactory {
  return (env, grants) => {
    let inner: Handler | null = null;
    let dead = false;
    let ready: Promise<Handler | null> | null = null;
    const get = () => ready ??= load().then(
      make => (dead ? null : (inner = make(env, grants))),
      e => {
        ready = null;
        throw new CapError("capability_disabled", `this capability could not load: ${e instanceof Error ? e.message : String(e)}`);
      },
    );
    return {
      async call(method, args) {
        const h = await get();
        if (!h) throw new CapError("capability_disabled", "this view has ended");
        return h.call(method, args);
      },
      onEvent: e => inner?.onEvent?.(e),
      reset: () => inner?.reset?.(),
      leave: () => inner?.leave?.(),
      uiChanged: () => inner?.uiChanged?.(),
      reveal: id => inner?.reveal?.(id) ?? false,
      dispose() {
        dead = true;
        if (inner?.dispose) inner.dispose();
        else inner?.reset?.();
        inner = null;
      },
    };
  };
}
```

In `web/shell/src/caps/host.ts`, `Handler` gains

```ts
  /** The frame's document left (a `bye`, a load without a hello, a hello the
   * shell does not welcome, or a navigation the shell started): close what
   * serves that document live (a socket, a stream). State the next hello
   * resets with `reset` may stay until then. */
  leave?(): void;
```

and `CapabilityHost` gains, after `reset()`:

```ts
  /** The frame's document left; see `Handler.leave`. */
  leave(): void {
    if (this.dead) return;
    for (const h of this.handlers.values()) h.leave?.();
  }
```

`web/shell/src/caps/room.ts`:

```ts
// The `room` capability's shell side: one WebSocket per open document to
// `/api/artifacts/<aid>/room?peer=<label>`, plus `&token=<bearer>` when this
// shell holds the token (the daemon's frames and levels are in
// docs/contract.md "Room protocol"), in both frame modes, relayed to the page
// as `clax:event`s of ns "room". The label is kept across reconnects and
// replaced when the frame loads a new document. On reconnect the shell
// re-sends the page's latest presence and re-joins its rooms; the page
// re-sends nothing. The daemon fixes a socket's level when it opens, so when
// the viewer's name changes (a LAN viewer who names themselves moves from
// `view` to `interact`) the shell opens a new socket under the same label,
// which replaces the old one without the page seeing it leave. This module is
// a lazy chunk (registry.ts), loaded on the page's first room call.
import { onViewer } from "../threads";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

export type SocketLike = {
  readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((e: unknown) => void) | null;
  onclose: ((e: { code: number; reason: string }) => void) | null;
  onmessage: ((e: { data: unknown }) => void) | null;
};
export type OpenSocket = (url: string) => SocketLike;
export type Timer = (fn: () => void, ms: number) => ReturnType<typeof setTimeout>;

/** Delays between reconnect attempts; the last repeats. */
export const RECONNECT_MS = [1000, 2000, 5000];
export const ACK_TIMEOUT_MS = 10_000;

const ALPHABET = "0123456789abcdefghijklmnopqrstuv";

/** Sixteen random characters of [0-9a-v]: one open document's peer label. */
export function peerLabel(): string {
  return [...crypto.getRandomValues(new Uint8Array(16))].map(b => ALPHABET[b & 31]).join("");
}

type Frame = { t: string; id?: number; room?: string | null; code?: string; message?: string; dropped?: boolean; [k: string]: unknown };
type Waiter = { resolve(f: Frame): void; reject(e: unknown): void; timer: ReturnType<typeof setTimeout> };

export type Viewers = (fn: (v: unknown) => void) => () => void;

export function makeRoomHandler(
  open: OpenSocket = url => new WebSocket(url) as unknown as SocketLike,
  timer: Timer = (fn, ms) => setTimeout(fn, ms),
  viewers: Viewers = onViewer as Viewers,
): HandlerFactory {
  return env => {
    let label = peerLabel();
    let ws: SocketLike | null = null;
    let isOpen = false;
    let wanted = false;
    let terminal = false;
    let attempt = 0;
    let seq = 0;
    let generation = 0;
    const presence = new Map<string | null, unknown>();
    const joined = new Set<string>();
    const waiting = new Map<number, Waiter>();
    // The socket a rename replaced: closed once its successor opens.
    let retired: SocketLike | null = null;

    const push = (topic: string, data: unknown) => env.post({ type: "clax:event", ns: "room", topic, data });
    // The owner shell appends the token: a WebSocket cannot send an Authorization header.
    const url = () => `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/artifacts/${env.aid}/room?peer=${label}`
      + (env.token ? `&token=${encodeURIComponent(env.token)}` : "");

    function failWaiters(message: string) {
      for (const w of waiting.values()) { clearTimeout(w.timer); w.reject(new CapError("upstream_error", message)); }
      waiting.clear();
    }

    function request(msg: Record<string, unknown>): Promise<Frame> {
      if (!ws || !isOpen) return Promise.reject(new CapError("upstream_error", "the room is not connected"));
      const id = ++seq;
      const s = ws;
      return new Promise((resolve, reject) => {
        const t = timer(() => { waiting.delete(id); reject(new CapError("upstream_error", "the room did not answer within 10 s")); }, ACK_TIMEOUT_MS);
        waiting.set(id, { resolve, reject, timer: t });
        s.send(JSON.stringify({ ...msg, id }));
      });
    }

    function rejoin(room: string) {
      request({ t: "join", room }).catch(e => {
        joined.delete(room);
        push("error", { room, code: e instanceof CapError ? e.code : "upstream_error", message: e instanceof Error ? e.message : String(e) });
      });
    }

    function onFrame(f: Frame) {
      switch (f.t) {
        case "welcome": push("welcome", { peer: f.peer }); return;
        case "peers": case "peer": case "left": case "msg": push(f.t, f); return;
        case "ack": case "nack": {
          const w = waiting.get(Number(f.id));
          if (!w) return;
          waiting.delete(Number(f.id));
          clearTimeout(w.timer);
          if (f.t === "ack") w.resolve(f); else w.reject(new CapError(String(f.code), String(f.message)));
          return;
        }
        default: return;
      }
    }

    function connect() {
      if (!wanted || terminal || ws) return;
      const mine = generation;
      const s = open(url());
      ws = s;
      s.onopen = () => {
        if (ws !== s) return;
        retired?.close(1000, "renamed");
        retired = null;
        isOpen = true;
        attempt = 0;
        push("connection", { connected: true });
        for (const [room, state] of presence) s.send(JSON.stringify({ t: "presence", room, state }));
        for (const room of joined) rejoin(room);
      };
      s.onmessage = e => {
        if (ws !== s) return;
        let f: Frame;
        try { f = JSON.parse(String(e.data)) as Frame; } catch { return; }
        onFrame(f);
      };
      s.onclose = e => {
        if (ws !== s) return;
        ws = null;
        const was = isOpen;
        isOpen = false;
        failWaiters("the room connection closed");
        if (was) push("connection", { connected: false });
        if (e.code === 4403) {
          terminal = true;
          const code = e.reason === "revoked" ? "revoked" : "not_granted";
          push("error", { room: null, code, message: code === "revoked" ? "this view's access to the room was withdrawn" : "this view cannot join the room" });
          return;
        }
        if (e.code === 4409) { terminal = true; return; }
        if (!wanted || mine !== generation) return;
        const delay = RECONNECT_MS[Math.min(attempt++, RECONNECT_MS.length - 1)];
        timer(() => { if (mine === generation) connect(); }, delay);
      };
    }

    function end() {
      generation++;
      wanted = false;
      terminal = false;
      attempt = 0;
      const s = ws;
      ws = null;
      isOpen = false;
      failWaiters("the page went away");
      s?.close(1000, "left");
      retired?.close(1000, "left");
      retired = null;
      presence.clear();
      joined.clear();
      label = peerLabel();
    }

    const offViewer = viewers(() => {
      if (!wanted || terminal || !ws) return;
      // Detached first: its close (4409 from the daemon, or ours) is not news for the page.
      const old = ws;
      old.onopen = old.onmessage = old.onclose = null;
      retired = old;
      ws = null;
      isOpen = false;
      failWaiters("the room reconnected");
      connect();
    });

    return {
      async call(method, args) {
        switch (method) {
          case "connect":
            wanted = true;
            connect();
            return null;
          case "presence": {
            const room = (args[0] ?? null) as string | null;
            presence.set(room, args[1]);
            if (ws && isOpen) ws.send(JSON.stringify({ t: "presence", room, state: args[1] }));
            return null;
          }
          case "emit": {
            if (!ws || !isOpen) return { dropped: false };
            const [room, topic] = args;
            const msg: Record<string, unknown> = { t: "emit", room: room ?? null, topic };
            if (args.length > 2) msg.data = args[2];
            const f = await request(msg);
            return { dropped: f.dropped === true };
          }
          case "join": {
            const room = String(args[0]);
            await request({ t: "join", room });
            joined.add(room);
            return null;
          }
          case "leave": {
            const room = String(args[0]);
            joined.delete(room);
            presence.delete(room);
            if (ws && isOpen) ws.send(JSON.stringify({ t: "leave", room }));
            return null;
          }
          default:
            throw new CapError("capability_removed", `room.${method} is not part of this runtime`);
        }
      },
      reset: end,
      leave: end,
      dispose() {
        offViewer();
        end();
      },
    };
  };
}

export const roomHandler: HandlerFactory = makeRoomHandler();
```

`web/shell/src/caps/registry.ts`: add

```ts
import { lazyHandler } from "./lazy";

// …in REGISTRY:
  room: lazyHandler(() => import("./room").then(m => m.roomHandler)),
```

`web/shell/src/caps/availability.ts`: `CAPABILITIES` gains `"room"` after `"assets"`; `isAvailable` gains `case "room": return declares(name, declared);`; the header comment's last clause becomes "`room` when declared; files, mcp, and sample never" (Task 8 changes `sample`).

- [ ] **Step 4: Leave the document wherever the gate closes**

In `web/shell/src/view/artifact-controller.ts`, add:

```ts
  /** The frame's document is gone, or is not the page: what serves it live
   * (the room socket, sample streams) ends now, not at the next hello. */
  private leaveDocument(): void {
    this.host?.leave();
  }
```

and call it in `frameLeft()` (after `this.gate.bye()`), in `frameLoaded()`'s stale branch (`if (this.gate.load()) { this.leaveDocument(); … }`), in `navigateFrame()` (after `this.gate.close()`), and in the `clax:hello` case where the gate stays closed (`if (!this.gate.open) { this.leaveDocument(); this.set({ file: null }); break; }`).

- [ ] **Step 5: Run the shell tests to verify they pass**

Run: `cd web && npm test -- --reporter=dot && npm run typecheck && npm run lint`
Expected: PASS.

- [ ] **Step 6: Write the sample page and the failing browser tests**

`web/e2e/pages/room.html` (a claude.ai page: no mention of the host, theme tokens for both themes):

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Who's here</title>
<style>
:root { --bg: #ffffff; --fg: #1d1d1f; --muted: #6e6e73; --line: #d2d2d7; --accent: #0a66c2; }
@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { --bg: #161618; --fg: #f5f5f7; --muted: #a1a1a6; --line: #3a3a3c; --accent: #4ea1ff; } }
:root[data-theme="dark"] { --bg: #161618; --fg: #f5f5f7; --muted: #a1a1a6; --line: #3a3a3c; --accent: #4ea1ff; }
body { margin: 0; padding: 16px; background: var(--bg); color: var(--fg); font: 15px/1.5 system-ui, sans-serif; }
button { font: inherit; padding: 6px 12px; border: 1px solid var(--line); border-radius: 6px; background: transparent; color: var(--fg); cursor: pointer; }
button:hover { border-color: var(--accent); }
.muted { color: var(--muted); }
ul, ol { padding-left: 20px; }
</style>
</head>
<body>
<h1>Who's here</h1>
<p><span id="status" class="muted">offline</span> · <span id="count">0</span> here</p>
<p><button id="pick-a">Pick A</button> <button id="pick-b">Pick B</button> <button id="wave">Wave</button></p>
<ul id="peers"></ul>
<h2>Table</h2>
<p><button id="join">Join the table</button> <button id="leave">Leave the table</button> <span id="table-count">0</span> at the table</p>
<ol id="log"></ol>
<script>
(async () => {
  const room = await claude.use("room");
  const status = document.getElementById("status");
  if (!room) { status.textContent = "unavailable"; return; }
  const who = p => (p.sameTab ? "this tab" : p.isMe ? "your other tab" : "someone else");
  const log = text => { const li = document.createElement("li"); li.textContent = text; document.getElementById("log").append(li); };
  room.onConnection(c => { status.textContent = c ? "connected" : "offline"; });
  room.onPeers(change => {
    document.getElementById("count").textContent = String(change.peers.length);
    document.getElementById("peers").replaceChildren(...change.peers.map(p => {
      const li = document.createElement("li");
      li.dataset.peer = p.peer;
      li.textContent = who(p) + ": " + (typeof p.presence.pick === "string" ? p.presence.pick : "nothing");
      return li;
    }));
  });
  room.on("reaction", m => log("wave from " + who(m)));
  document.getElementById("pick-a").onclick = () => room.presence({ pick: "A" });
  document.getElementById("pick-b").onclick = () => room.presence({ pick: "B" });
  document.getElementById("wave").onclick = () => room.emit("reaction", { kind: "wave" }).catch(e => log("refused: " + e.code));
  let table = null;
  document.getElementById("join").onclick = async () => {
    table = await room.join("table-1");
    table.onPeers(change => { document.getElementById("table-count").textContent = String(change.peers.length); });
  };
  document.getElementById("leave").onclick = async () => {
    await table?.leave();
    table = null;
    document.getElementById("table-count").textContent = "0";
  };
})();
</script>
</body>
</html>
```

`web/e2e/room.spec.ts`:

```ts
import { readFileSync } from "node:fs";
import { test, expect } from "@playwright/test";
import { openArtifact, publishWith, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const PAGE = readFileSync(new URL("./pages/room.html", import.meta.url), "utf8");
const CAPS = { room: { topics: { reaction: "interact" } } };

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: two tabs see each other's presence, events, and named rooms`, async ({ context }) => {
    const { artifact } = await publishWith(d.base, d.token, `Room ${mode}`, PAGE, CAPS);
    const a = await context.newPage();
    const b = await context.newPage();
    const fa = await openArtifact(a, d.base, artifact.id, 1, mode);
    const fb = await openArtifact(b, d.base, artifact.id, 1, mode);
    await expect(fa.locator("#status")).toHaveText("connected");
    await expect(fb.locator("#status")).toHaveText("connected");
    await expect(fa.locator("#count")).toHaveText("2");
    await expect(fb.locator("#count")).toHaveText("2");

    await fa.locator("#pick-b").click();
    await expect(fa.locator("#peers")).toContainText("this tab: B");
    await expect(fb.locator("#peers")).toContainText("your other tab: B");

    await fb.locator("#wave").click();
    await expect(fa.locator("#log")).toContainText("wave from your other tab");
    await expect(fb.locator("#log")).toContainText("wave from this tab");

    await fa.locator("#join").click();
    await expect(fa.locator("#table-count")).toHaveText("1");
    await fb.locator("#join").click();
    await expect(fa.locator("#table-count")).toHaveText("2");
    await expect(fb.locator("#table-count")).toHaveText("2");
    await fa.locator("#leave").click();
    await expect(fb.locator("#table-count")).toHaveText("1");

    await b.close();
    await expect(fa.locator("#count")).toHaveText("1");
  });

  test(`${mode}: a reload is a new peer, and the old one leaves`, async ({ context }) => {
    const { artifact } = await publishWith(d.base, d.token, `Room reload ${mode}`, PAGE, CAPS);
    const a = await context.newPage();
    const b = await context.newPage();
    const fa = await openArtifact(a, d.base, artifact.id, 1, mode);
    await openArtifact(b, d.base, artifact.id, 1, mode);
    await expect(fa.locator("#count")).toHaveText("2");
    const before = await fa.locator("#peers li").evaluateAll(els => els.map(e => (e as HTMLElement).dataset.peer));
    await b.reload();
    await expect.poll(async () => {
      const now = await fa.locator("#peers li").evaluateAll(els => els.map(e => (e as HTMLElement).dataset.peer));
      return now.length === 2 && now.some(p => !before.includes(p));
    }, { timeout: 15_000 }).toBe(true);
  });
}

/** A foreign document that greets nobody and keeps every message it receives. */
const RECORDER = "<!doctype html><title>Elsewhere</title><script>window.got = []; addEventListener(\"message\", e => window.got.push(e.data));</script><p>elsewhere</p>";

test("sandbox: a document that leaves takes its peer with it, and nothing of the room reaches the next one", async ({ context }) => {
  const { artifact } = await publishWith(d.base, d.token, "Room leave", PAGE, CAPS);
  const a = await context.newPage();
  const b = await context.newPage();
  await a.route("http://foreign.test/**", r => r.fulfill({ contentType: "text/html", body: RECORDER }));
  const fa = await openArtifact(a, d.base, artifact.id, 1, "sandbox");
  const fb = await openArtifact(b, d.base, artifact.id, 1, "sandbox");
  await expect(fa.locator("#count")).toHaveText("2");
  await expect(fb.locator("#count")).toHaveText("2");
  // The page sends its own frame elsewhere; B stays quiet until A's peer has left.
  await fa.evaluate(() => { location.href = "http://foreign.test/elsewhere"; });
  await expect(fb.locator("#count")).toHaveText("1");
  await fb.locator("#pick-b").click();
  await fb.locator("#wave").click();
  await expect(fb.locator("#log")).toContainText("wave from this tab");
  await expect.poll(() => a.frame({ url: /foreign\.test/ }) !== null).toBe(true);
  const rec = a.frame({ url: /foreign\.test/ })!;
  // Long enough for B's presence and wave to have reached A's shell, were its socket still open.
  await a.waitForTimeout(1000);
  const got = await rec.evaluate(() => (window as unknown as { got: { type?: string }[] }).got);
  expect(got.filter(m => m && typeof m === "object" && String(m.type).startsWith("clax:"))).toEqual([]);
});

test("lan: naming yourself reconnects the room at the new level, so an interact topic opens", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Room lan", PAGE, CAPS);
  const f = await openArtifact(page, d.base, artifact.id, 1, "sandbox", { lan: true });
  await expect(f.locator("#status")).toHaveText("connected");
  await f.locator("#wave").click();
  await expect(f.locator("#log")).toContainText("refused: not_permitted");
  // The name field's place is Echo's (align with Echo at merge): on main it is in the top bar.
  await page.getByLabel("Your name").fill("Ben");
  await page.getByLabel("Your name").press("Enter");
  await expect(async () => {
    await f.locator("#wave").click();
    await expect(f.locator("#log")).toContainText("wave from this tab", { timeout: 1000 });
  }).toPass({ timeout: 15_000 });
  await expect(f.locator("#count")).toHaveText("1");
});
```

In `web/e2e/contract.spec.ts`, add to `CASES`:

```ts
  "room.html": {
    caps: { room: { topics: { reaction: "interact" } } },
    async check(f) {
      await expect(f.locator("#status")).toHaveText("connected");
      await expect(f.locator("#count")).toHaveText("1");
      await f.locator("#pick-a").click();
      await expect(f.locator("#peers")).toHaveText("this tab: A");
      await f.locator("#wave").click();
      await expect(f.locator("#log")).toHaveText("wave from this tab");
    },
  },
```

- [ ] **Step 7: Run the browser tests and the time-to-usable gates**

Run: `cd web && npm run build && npx playwright test room.spec.ts contract.spec.ts capabilities.spec.ts && node scripts/bundle-size.mjs && npm run perf`
Expected: PASS in both modes. `bundle-size.mjs` shows `artifact` unchanged from before this task (the room handler is a dynamic chunk outside the entry's static closure; if `artifact` grew, a static import of `./room` crept in); `perf` passes its budgets unchanged.

Then by hand: start a scratch daemon (`H=$(mktemp -d); CLAX_HOME=$H CLAX_CODEX_BIN= cargo run -p clax-cli -- serve --foreground --bind 127.0.0.1 --port 0`, and read the port from `$H/daemon.json`), publish `room.html` with `{"room": {"topics": {"reaction": "interact"}}}` through `POST /api/artifacts` with the token from `$H/daemon.json`, open it in two tabs, pick and wave in each, and check light mode, dark mode and phone width; stop the daemon (`CLAX_HOME=$H cargo run -p clax-cli -- stop`) and start it again on the same home and port (`--port <the port it had>`) within a few seconds, and watch one tab's status go `offline` and back to `connected`. Remove `$H` afterwards.

- [ ] **Step 8: Stage**

Stage the change (`git add web/shell web/e2e`); do not commit. The controller commits it with the message:

```text
Relay room through one shell-owned socket per document, loaded on first use, closed when the document leaves, and reopened at a rename
```

---

### Task 7: `sample`'s page side as a lazy bridge part

The page-side `sample` module (argument checks, image preparation, streaming through `onText`, page tools, Stop) is carried over from the earlier draft of this plan, renamed. It now lives in a part of its own, and builds its callable namespace there, so the eager `capabilities.ts` gains no callable-namespace code (scan F5).

**Files:**
- Create: `web/bridge/src/caps/sample.ts`, `web/bridge/src/parts/sample.ts`
- Modify: `web/bridge/src/bridge.ts` (the `sample` part and arm), `web/bridge/src/parts/types.ts` (`SamplePart`; `Parts.sample`), `web/bridge/src/parts-url.ts` and `web/bridge/src/parts-static.ts` (`sample`), `web/scripts/build-parts.mjs`, `web/vite.bridge.config.ts`, `web/scripts/bundle-size.mjs` (`partSample`), `web/perf/bundle-budget.json` (`partSample`)
- Test: `web/bridge/test/sample.test.ts` (new); `web/bridge/test/capabilities.test.ts`, `web/bridge/test/use.test.ts` (updated)

**Interfaces:**
- Consumes: Task 5's part machinery (`PART_CAPABILITIES` already names `sample`); `Rpc.{call, on}`; the Sample relay of the Shared contract.
- Produces: `caps/sample.ts`: `makeSample(rpc)`, `sampleNamespace(rpc): unknown` (a frozen function with `json` and `limits`), `SAMPLE_METHODS = ["json", "limits"]`, `validate(input, options, verb)`, constants `MAX_PROMPT_BYTES = 65_536`, `TEXT_INTERVAL_MS = 100`, `TOOL_TIMEOUT_MS = 150_000`, `MAX_TOOL_RESULT_BYTES = 32_768`, `TARGET_PIXELS = 1_200_000`; `parts/sample.ts` exports `sampleNamespace` and `SAMPLE_METHODS`; `type SamplePart`; `Parts.sample(attempt?)`; budget key `partSample`.

- [ ] **Step 1: Write the failing bridge tests**

`web/bridge/test/sample.test.ts`:

```ts
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TEXT_INTERVAL_MS, sampleNamespace } from "../src/caps/sample";
import { CapabilityError } from "../src/rpc";

type Listener = (d: unknown) => void;
type Frame = { call: string; event: string; data: unknown };

function fakeRpc(run?: (call: string, req: Record<string, unknown>) => unknown) {
  const listeners = new Set<Listener>();
  const calls: { method: string; args: unknown[] }[] = [];
  const frame = (f: Frame) => { for (const l of [...listeners]) l(f); };
  return {
    calls,
    frame,
    rpc: {
      call: vi.fn(async (_ns: string, method: string, args: unknown[]) => {
        calls.push({ method, args });
        if (method === "run") return run ? run(args[0] as string, args[1] as Record<string, unknown>) : null;
        if (method === "limits") return { maxPromptBytes: 65536, tools: { maxCount: 16 } };
        return null;
      }),
      on: (_ns: string, _topic: string, f: Listener) => { listeners.add(f); return () => { listeners.delete(f); }; },
    },
  };
}

type SampleFn = ((input: unknown, options?: unknown) => Promise<unknown>) & { json(i: unknown, o?: unknown): Promise<unknown>; limits(): Promise<unknown> };

function setup(run?: (call: string, req: Record<string, unknown>) => unknown) {
  const f = fakeRpc(run);
  const sample = sampleNamespace(f.rpc as never) as unknown as SampleFn;
  const lastRun = () => f.calls.filter(c => c.method === "run").at(-1)!;
  return { ...f, sample, lastRun };
}

describe("sample", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("is a frozen function with json and limits", () => {
    const { sample } = setup();
    expect(typeof sample).toBe("function");
    expect(Object.isFrozen(sample)).toBe(true);
    expect(Object.keys(sample).sort()).toEqual(["json", "limits"]);
  });

  it("sends on the next microtask, streams whole text through onText, and resolves the result", async () => {
    const { sample, frame, calls, lastRun } = setup();
    const seen: { text: string; delta: string }[] = [];
    const p = sample("Summarize", { onText: (u: { text: string; delta: string }) => seen.push(u) });
    expect(calls).toEqual([]);
    await vi.advanceTimersByTimeAsync(0);
    const [call, req] = lastRun().args as [string, Record<string, unknown>];
    expect(req).toEqual({ input: "Summarize", verb: "text", model_tier: "default", tools: [], images: [], cache: true });
    frame({ call, event: "start", data: { call_id: "c", cached: false } });
    frame({ call, event: "text", data: { delta: " " } });
    frame({ call, event: "text", data: { delta: "Hel" } });
    expect(seen).toEqual([]);
    await vi.advanceTimersByTimeAsync(TEXT_INTERVAL_MS);
    frame({ call, event: "text", data: { delta: "lo" } });
    frame({ call: "someone-else", event: "text", data: { delta: "x" } });
    frame({ call, event: "done", data: { text: " Hello", truncated: false, model_tier_applied: "default" } });
    await expect(p).resolves.toEqual({ text: " Hello", truncated: false, modelTierApplied: "default" });
    expect(seen).toEqual([{ text: " Hel", delta: " Hel" }, { text: " Hello", delta: "lo" }]);
  });

  it("a cached answer is one onText call with the whole text", async () => {
    const { sample, frame, lastRun } = setup();
    const seen: unknown[] = [];
    const p = sample("q", { onText: (u: unknown) => seen.push(u) });
    await vi.advanceTimersByTimeAsync(0);
    const call = lastRun().args[0] as string;
    frame({ call, event: "start", data: { cached: true } });
    frame({ call, event: "done", data: { text: "whole", truncated: false, model_tier_applied: "quick" } });
    await p;
    expect(seen).toEqual([{ text: "whole", delta: "whole" }]);
  });

  it("an abort in the same block sends nothing; a later abort rejects cancelled with the kept text", async () => {
    const { sample, frame, calls, lastRun } = setup();
    const early = new AbortController();
    const p1 = sample("q", { signal: early.signal });
    early.abort();
    await expect(p1).rejects.toEqual({ code: "cancelled", message: expect.any(String) });
    expect(calls).toEqual([]);

    const ctl = new AbortController();
    const seen: string[] = [];
    const p2 = sample("q", { signal: ctl.signal, onText: ({ text }: { text: string }) => seen.push(text) });
    await vi.advanceTimersByTimeAsync(0);
    const call = lastRun().args[0] as string;
    frame({ call, event: "text", data: { delta: "tick " } });
    await vi.advanceTimersByTimeAsync(TEXT_INTERVAL_MS);
    frame({ call, event: "text", data: { delta: "tick " } });
    ctl.abort();
    await expect(p2).rejects.toEqual({ code: "cancelled", message: expect.any(String), text: "tick " });
    expect(calls.at(-1)).toEqual({ method: "cancel", args: [call] });
    frame({ call, event: "text", data: { delta: "more" } });
    await vi.advanceTimersByTimeAsync(TEXT_INTERVAL_MS);
    expect(seen).toEqual(["tick "]);
  });

  it("a reused aborted signal rejects at once", async () => {
    const { sample, calls } = setup();
    const ctl = new AbortController();
    ctl.abort();
    await expect(sample("q", { signal: ctl.signal })).rejects.toMatchObject({ code: "cancelled" });
    expect(calls).toEqual([]);
  });

  it("errors carry the streamed text, except refused", async () => {
    const { sample, frame, lastRun } = setup();
    const p = sample("q");
    await vi.advanceTimersByTimeAsync(0);
    let call = lastRun().args[0] as string;
    frame({ call, event: "text", data: { delta: "partial " } });
    frame({ call, event: "error", data: { code: "upstream_error", message: "broke" } });
    await expect(p).rejects.toEqual({ code: "upstream_error", message: "broke", text: "partial " });
    const r = sample("q");
    await vi.advanceTimersByTimeAsync(0);
    call = lastRun().args[0] as string;
    frame({ call, event: "text", data: { delta: "I won" } });
    frame({ call, event: "error", data: { code: "refused", message: "no" } });
    await expect(r).rejects.toEqual({ code: "refused", message: "no" });
  });

  it("a refusal before the stream (consent, cap) rejects with that code", async () => {
    const { sample } = setup(() => { throw new CapabilityError("not_granted", "declined"); });
    const p = sample("q");
    const check = expect(p).rejects.toEqual({ code: "not_granted", message: "declined" });
    await vi.advanceTimersByTimeAsync(0);
    await check;
  });

  it("json resolves the parsed value and rejects invalid_json with the raw reply", async () => {
    const { sample, frame, lastRun } = setup();
    const p = sample.json("q");
    await vi.advanceTimersByTimeAsync(0);
    let call = lastRun().args[0] as string;
    expect((lastRun().args[1] as { verb: string }).verb).toBe("json");
    frame({ call, event: "done", data: { text: "[1]", truncated: false, model_tier_applied: "default", value: [1] } });
    await expect(p).resolves.toEqual([1]);
    const bad = sample.json("q");
    await vi.advanceTimersByTimeAsync(0);
    call = lastRun().args[0] as string;
    frame({ call, event: "text", data: { delta: "nope" } });
    frame({ call, event: "error", data: { code: "invalid_json", message: "no JSON" } });
    await expect(bad).rejects.toEqual({ code: "invalid_json", message: "no JSON", text: "nope" });
  });

  it("runs page tools and posts their results, errors included", async () => {
    const { sample, frame, calls, lastRun } = setup();
    const execute = vi.fn(async (input: { shade?: unknown }) => ({ color: `teal-${String(input.shade)}` }));
    const boom = vi.fn(() => { throw new Error("no such track"); });
    const p = sample("q", { tools: [
      { name: "getColor", description: "Returns the colour.", inputSchema: { type: "object", properties: { shade: { type: "string" } } }, execute },
      { name: "boom", description: "Fails.", execute: boom },
    ] });
    await vi.advanceTimersByTimeAsync(0);
    const [call, req] = lastRun().args as [string, { tools: unknown; cache: unknown }];
    expect(req.tools).toEqual([
      { name: "getColor", description: "Returns the colour.", input_schema: { type: "object", properties: { shade: { type: "string" } } } },
      { name: "boom", description: "Fails." },
    ]);
    expect(req.cache).toBe(false);
    frame({ call, event: "tool_call", data: { id: "t1", name: "getColor", input: { shade: "dark" } } });
    frame({ call, event: "tool_call", data: { id: "t2", name: "boom", input: {} } });
    frame({ call, event: "tool_call", data: { id: "t3", name: "missing", input: {} } });
    await vi.advanceTimersByTimeAsync(0);
    expect(execute.mock.calls[0][1]).toHaveProperty("signal");
    const results = calls.filter(c => c.method === "toolResult").map(c => c.args).sort((a, b) => String(a[1]).localeCompare(String(b[1])));
    expect(results).toEqual([
      [call, "t1", JSON.stringify({ color: "teal-dark" }), false],
      [call, "t2", "Error: no such track", true],
      [call, "t3", "Error: no tool named missing", true],
    ]);
    frame({ call, event: "done", data: { text: "ok", truncated: false, model_tier_applied: "default" } });
    await p;
  });

  it("a tool's signal aborts when the call settles", async () => {
    const { sample, frame, lastRun } = setup();
    let signal: AbortSignal | null = null;
    const p = sample("q", { tools: [{ name: "slow", description: "Waits.", execute: (_i: unknown, ctx: { signal: AbortSignal }) => { signal = ctx.signal; return new Promise(() => {}); } }] });
    await vi.advanceTimersByTimeAsync(0);
    const call = lastRun().args[0] as string;
    frame({ call, event: "tool_call", data: { id: "t1", name: "slow", input: {} } });
    await vi.advanceTimersByTimeAsync(0);
    expect(signal!.aborted).toBe(false);
    frame({ call, event: "error", data: { code: "upstream_error", message: "x" } });
    await p.catch(() => {});
    expect(signal!.aborted).toBe(true);
  });

  it("rejects malformed calls with invalid_request or prompt_too_large, sending nothing", async () => {
    const { sample, calls } = setup();
    const noop = () => 1;
    for (const [input, options] of [
      ["", undefined],
      [{ prompt: "q" }, undefined],
      [[], undefined],
      [[{ role: "assistant", content: "a" }], undefined],
      [[{ role: "user", content: "q" }, { role: "assistant", content: "a" }], undefined],
      [[{ role: "system", content: "q" }], undefined],
      ["q", "not an object"],
      ["q", new AbortController()],
      ["q", { signal: new AbortController() }],
      ["q", { onText: "x" }],
      ["q", { modelTier: "huge" }],
      ["q", { cache: "yes" }],
      ["q", { cache: { gcTime: 0 } }],
      ["q", { tools: "x" }],
      ["q", { tools: [{ name: "bad name", description: "d", execute: noop }] }],
      ["q", { tools: [{ name: "a", description: "d", execute: noop }, { name: "a", description: "d", execute: noop }] }],
      ["q", { tools: [{ name: "a", description: "", execute: noop }] }],
      ["q", { tools: [{ name: "a", description: "d" }] }],
      ["q", { tools: [{ name: "a", description: "d", inputSchema: { type: "array" }, execute: noop }] }],
      ["q", { tools: [{ name: "a", description: "d", execute: noop }], cache: true }],
      ["q", { images: "not a blob" }],
    ] as [unknown, unknown][]) {
      await expect(sample(input, options), JSON.stringify([input, String(options)])).rejects.toMatchObject({ code: "invalid_request" });
    }
    await expect(sample("x".repeat(65_537))).rejects.toMatchObject({ code: "prompt_too_large" });
    expect(calls).toEqual([]);
  });

  it("maps cache options to the wire and ignores unknown option members", async () => {
    const { sample, lastRun } = setup();
    void sample("q", { cache: { gcTime: 1e12, refresh: true }, somethingNew: 1 });
    await vi.advanceTimersByTimeAsync(0);
    expect((lastRun().args[1] as { cache: unknown }).cache).toEqual({ gc_time_ms: 86_400_000, refresh: true });
    void sample("q", { cache: false, modelTier: "quick" });
    await vi.advanceTimersByTimeAsync(0);
    expect(lastRun().args[1]).toMatchObject({ cache: false, model_tier: "quick" });
  });

  it("limits comes from the shell", async () => {
    const { sample } = setup();
    await expect(sample.limits()).resolves.toEqual({ maxPromptBytes: 65536, tools: { maxCount: 16 } });
  });
});
```

In `web/bridge/test/capabilities.test.ts`, import `SAMPLE_METHODS` from `../src/caps/sample` and add:

```ts
  it("sample (its part's own list; the call itself is the namespace)", () => {
    // `function json<T = unknown>(` is generic, so the scan reads `function name<` too.
    const declared = [...new Set([...contract("sample").matchAll(/^\s*function (\w+)[<(]/gm)].map(m => m[1]))].filter(n => n !== "sample").sort();
    expect([...SAMPLE_METHODS].sort()).toEqual(declared);
  });
```

In `web/bridge/test/use.test.ts`, the expectation of "aliases self to artifact…" becomes `["artifact", "room", "sample", "db"]`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && npx vitest run bridge/test`
Expected: FAIL (`../src/caps/sample` does not exist; `use("sample")` is not yet asked of the shell).

- [ ] **Step 3: Implement the module and its part**

`web/bridge/src/caps/sample.ts`:

```ts
// The `sample` capability (contract 0.2.61 sample.d.ts), page side, in the
// bridge's lazy `sample` part (parts/sample.ts): argument
// checks, image preparation, and one call's lifecycle over the shell relay
// (web/shell/src/caps/sample.ts): the request leaves on the next microtask,
// `onText` gets the whole text so far a few times a second, page tools run
// here and their results go back, an abort rejects `cancelled` with the text
// the page may keep. Every failure is one rejected {code, message, text?}.
import type { Rpc } from "../rpc";

export const MAX_PROMPT_BYTES = 65_536;
export const TEXT_INTERVAL_MS = 100;
export const TOOL_TIMEOUT_MS = 150_000;
export const MAX_TOOL_RESULT_BYTES = 32_768;
export const MAX_TOOL_DESCRIPTION_BYTES = 1024;
export const MAX_TOOL_SCHEMA_BYTES = 4096;
export const MAX_TOOLS = 16;
export const TARGET_PIXELS = 1_200_000;
const MAX_GC_MS = 86_400_000;
const IMAGE_TYPES = ["image/jpeg", "image/png", "image/webp", "image/gif"];
const MAX_INPUT_IMAGE_BYTES = 20_000_000;
const MAX_SIDE = 10_000;
const MAX_IMAGE_PIXELS = 64_000_000;
const TOOL_NAME = /^[A-Za-z0-9_-]{1,128}$/;
const TIERS = new Set(["default", "complex", "quick"]);
const KNOWN_OPTIONS = new Set(["onText", "signal", "tools", "images", "modelTier", "cache"]);

export type SampleError = { code: string; message: string; text?: string };
type Tool = { name: string; description: string; inputSchema?: Record<string, unknown>; execute: (input: Record<string, unknown>, ctx: { signal: AbortSignal }) => unknown };
type WireRequest = {
  input: string | { role: string; content: string }[];
  verb: "text" | "json";
  model_tier: string;
  tools: { name: string; description: string; input_schema?: unknown }[];
  images: { media_type: string; data: string }[];
  cache: boolean | { gc_time_ms?: number; refresh?: boolean };
};
type Checked = { req: WireRequest; onText?: (u: { text: string; delta: string }) => unknown; signal?: AbortSignal; tools: Tool[]; images: Blob[] };
type Frame = { call: string; event: "start" | "text" | "tool_call" | "done" | "error"; data: Record<string, unknown> };

const bytes = (s: string) => new TextEncoder().encode(s).length;
const err = (code: string, message: string, text?: string): SampleError => (text ? { code, message, text } : { code, message });
const bad = (message: string) => err("invalid_request", message);

function isPlainObject(v: unknown): v is Record<string, unknown> {
  if (v === null || typeof v !== "object" || Array.isArray(v)) return false;
  const p = Object.getPrototypeOf(v);
  return p === Object.prototype || p === null;
}

let warnedUnknown = false;

/** Checks a call's arguments; a SampleError says what to fix. */
export function validate(input: unknown, options: unknown, verb: "text" | "json"): Checked | SampleError {
  if (options !== undefined && !isPlainObject(options)) {
    if (options instanceof AbortController) return bad("options must be a plain object: pass { signal: controller.signal }");
    if (options instanceof Blob) return bad("options must be a plain object: pass images as { images: file }");
    return bad("options must be a plain object");
  }
  const o = (options ?? {}) as Record<string, unknown>;
  const unknown = Object.keys(o).filter(k => !KNOWN_OPTIONS.has(k));
  if (unknown.length && !warnedUnknown) { warnedUnknown = true; console.warn(`sample(): ignoring unknown options ${unknown.join(", ")}`); }

  let wireInput: WireRequest["input"];
  if (typeof input === "string") {
    if (!input.trim()) return bad("input is empty");
    if (bytes(input) > MAX_PROMPT_BYTES) return err("prompt_too_large", `the input is ${bytes(input)} bytes; the limit is ${MAX_PROMPT_BYTES}`);
    wireInput = input;
  } else if (Array.isArray(input)) {
    if (!input.length) return bad("input has no turns");
    const turns: { role: string; content: string }[] = [];
    for (const [i, t] of input.entries()) {
      if (!isPlainObject(t) || (t.role !== "user" && t.role !== "assistant")) return bad(`input[${i}].role must be "user" or "assistant"`);
      if (typeof t.content !== "string" || !t.content) return bad(`input[${i}].content must be a non-empty string`);
      turns.push({ role: t.role, content: t.content });
    }
    if (turns[0].role !== "user" || turns.at(-1)!.role !== "user") return bad("input turns must start and end with a user turn");
    const total = turns.reduce((n, t) => n + bytes(t.content), 0);
    if (total > MAX_PROMPT_BYTES) return err("prompt_too_large", `the input is ${total} bytes; the limit is ${MAX_PROMPT_BYTES}`);
    wireInput = turns;
  } else {
    return bad("input is a prompt string or an array of {role, content} turns (the prompt goes first; there is no {prompt} object form)");
  }

  if (o.onText !== undefined && typeof o.onText !== "function") return bad("onText must be a function");
  if (o.signal !== undefined && !(o.signal instanceof AbortSignal)) return bad("signal must be an AbortSignal: pass controller.signal, not the controller");
  const tier = o.modelTier ?? "default";
  if (typeof tier !== "string" || !TIERS.has(tier)) return bad(`modelTier is "default", "complex" or "quick", not ${JSON.stringify(tier)}`);

  const tools: Tool[] = [];
  if (o.tools !== undefined) {
    if (!Array.isArray(o.tools)) return bad("tools must be an array of tools");
    if (o.tools.length > MAX_TOOLS) return bad(`at most ${MAX_TOOLS} tools per call`);
    for (const [i, t] of o.tools.entries()) {
      if (!isPlainObject(t)) return bad(`tools[${i}] must be a plain object`);
      if (typeof t.name !== "string" || !TOOL_NAME.test(t.name)) return bad(`tools[${i}].name must be 1-128 of A-Z a-z 0-9 _ -`);
      if (tools.some(x => x.name === t.name)) return bad(`tools[${i}].name '${t.name}' is used twice`);
      if (typeof t.description !== "string" || !t.description.trim() || bytes(t.description) > MAX_TOOL_DESCRIPTION_BYTES) return bad(`tools[${i}].description must be 1-${MAX_TOOL_DESCRIPTION_BYTES} bytes`);
      if (t.inputSchema !== undefined && (!isPlainObject(t.inputSchema) || t.inputSchema.type !== "object" || bytes(JSON.stringify(t.inputSchema)) > MAX_TOOL_SCHEMA_BYTES)) {
        return bad(`tools[${i}].inputSchema must be a JSON Schema object of type "object", at most ${MAX_TOOL_SCHEMA_BYTES} bytes`);
      }
      if (typeof t.execute !== "function") return bad(`tools[${i}].execute must be a function`);
      tools.push({ name: t.name, description: t.description, inputSchema: t.inputSchema as Record<string, unknown> | undefined, execute: t.execute as Tool["execute"] });
    }
  }

  let cache: WireRequest["cache"];
  if (o.cache === undefined) cache = tools.length === 0;
  else if (o.cache === true || o.cache === false) cache = o.cache;
  else if (isPlainObject(o.cache)) {
    const { gcTime, refresh } = o.cache;
    if (gcTime !== undefined && (typeof gcTime !== "number" || !Number.isFinite(gcTime) || gcTime <= 0)) return bad("cache.gcTime must be a number of milliseconds greater than zero");
    if (refresh !== undefined && typeof refresh !== "boolean") return bad("cache.refresh must be true or false");
    cache = { ...(gcTime === undefined ? {} : { gc_time_ms: Math.min(gcTime, MAX_GC_MS) }), ...(refresh === undefined ? {} : { refresh }) };
  } else return bad("cache must be true, false, or {gcTime?, refresh?}");
  if (tools.length && cache !== false) return bad("a call with tools is never cached: omit cache or pass false");

  let images: Blob[] = [];
  if (o.images !== undefined) {
    const list = o.images instanceof Blob ? [o.images] : Array.isArray(o.images) ? o.images : typeof FileList !== "undefined" && o.images instanceof FileList ? [...o.images] : null;
    if (!list || !list.every(b => b instanceof Blob)) return bad("images must be a Blob, a File, an array of them, or a FileList");
    images = list as Blob[];
  }

  return {
    req: {
      input: wireInput, verb, model_tier: tier, cache, images: [],
      tools: tools.map(t => (t.inputSchema === undefined ? { name: t.name, description: t.description } : { name: t.name, description: t.description, input_schema: t.inputSchema })),
    },
    onText: o.onText as Checked["onText"], signal: o.signal as AbortSignal | undefined, tools, images,
  };
}

async function base64(blob: Blob): Promise<string> {
  const buf = new Uint8Array(await blob.arrayBuffer());
  let s = "";
  for (let i = 0; i < buf.length; i += 0x8000) s += String.fromCharCode(...buf.subarray(i, i + 0x8000));
  return btoa(s);
}

/** Downsizes each image to about 1.2 megapixels, applies orientation, keeps an
 * animation's first frame, and drops metadata (a canvas re-encode). */
async function prepareImages(list: Blob[]): Promise<WireRequest["images"]> {
  const out: WireRequest["images"] = [];
  for (const b of list) {
    if (!IMAGE_TYPES.includes(b.type)) throw err("image_rejected", `images are JPEG, PNG, WebP or GIF, not '${b.type || "unknown"}'`);
    if (b.size > MAX_INPUT_IMAGE_BYTES) throw err("image_rejected", "an image is over 20 MB");
    let bmp: ImageBitmap;
    try { bmp = await createImageBitmap(b, { imageOrientation: "from-image" }); } catch { throw err("image_rejected", "an image could not be decoded"); }
    if (bmp.width > MAX_SIDE || bmp.height > MAX_SIDE || bmp.width * bmp.height > MAX_IMAGE_PIXELS) throw err("image_rejected", "an image is over 10,000 px a side or 64 megapixels");
    const scale = Math.min(1, Math.sqrt(TARGET_PIXELS / (bmp.width * bmp.height)));
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(bmp.width * scale));
    canvas.height = Math.max(1, Math.round(bmp.height * scale));
    canvas.getContext("2d")!.drawImage(bmp, 0, 0, canvas.width, canvas.height);
    const type = b.type === "image/png" || b.type === "image/gif" ? "image/png" : "image/jpeg";
    const encoded = await new Promise<Blob | null>(r => canvas.toBlob(r, type, 0.9));
    if (!encoded) throw err("image_rejected", "an image could not be re-encoded");
    out.push({ media_type: type, data: await base64(encoded) });
  }
  return out;
}

export function makeSample(rpc: Pick<Rpc, "call" | "on">): Record<string, (...args: never[]) => unknown> {
  let seq = 0;

  function run(verb: "text" | "json", input: unknown, options: unknown): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const checked = validate(input, options, verb);
      if ("code" in checked) { reject(checked); return; }
      const v = checked;
      const call = `s${++seq}`;
      const tools = new Map(v.tools.map(t => [t.name, t]));
      const running = new Set<AbortController>();
      let text = "";
      let shown = "";
      let settled = false;
      let started = false;
      let timer: ReturnType<typeof setTimeout> | null = null;

      const kept = () => (v.onText ? shown : text) || undefined;
      const flushText = () => {
        timer = null;
        if (settled || !v.onText || text === shown || !text.trim()) return;
        const delta = text.slice(shown.length);
        shown = text;
        try {
          const r = v.onText({ text, delta }) as { then?: unknown; catch?: (f: (e: unknown) => void) => void } | undefined;
          if (r && typeof r.then === "function" && typeof r.catch === "function") r.catch(e => console.error(e));
        } catch (e) { console.error(e); }
      };
      const finish = (ok: boolean, value: unknown) => {
        if (settled) return;
        if (ok) flushText();
        settled = true;
        if (timer) clearTimeout(timer);
        off();
        v.signal?.removeEventListener("abort", onAbort);
        for (const c of running) c.abort();
        if (ok) resolve(value); else reject(value);
      };
      const onAbort = () => {
        if (started) void rpc.call("sample", "cancel", [call]).catch(() => {});
        finish(false, err("cancelled", "the call's signal aborted", kept()));
      };

      async function runTool(tc: { id: string; name: string; input: unknown }) {
        const ctl = new AbortController();
        running.add(ctl);
        const t = setTimeout(() => ctl.abort(), TOOL_TIMEOUT_MS);
        let content = "";
        let isError = false;
        try {
          const tool = tools.get(tc.name);
          if (!tool) throw new Error(`no tool named ${tc.name}`);
          const arg = isPlainObject(tc.input) ? tc.input : {};
          const out = await tool.execute(arg, { signal: ctl.signal });
          content = typeof out === "string" ? out : JSON.stringify(out ?? null);
          if (bytes(content) > MAX_TOOL_RESULT_BYTES) { content = `Error: the tool's result is over ${MAX_TOOL_RESULT_BYTES} bytes`; isError = true; }
        } catch (e) {
          content = `Error: ${e instanceof Error ? e.message : String(e)}`;
          isError = true;
        } finally {
          clearTimeout(t);
          running.delete(ctl);
        }
        if (!settled) void rpc.call("sample", "toolResult", [call, tc.id, content, isError]).catch(() => {});
      }

      const off = rpc.on("sample", "frame", d => {
        const f = d as Frame;
        if (f.call !== call || settled) return;
        switch (f.event) {
          case "text":
            text += String(f.data.delta ?? "");
            if (!timer) timer = setTimeout(flushText, TEXT_INTERVAL_MS);
            return;
          case "tool_call":
            void runTool(f.data as { id: string; name: string; input: unknown });
            return;
          case "done":
            text = String(f.data.text ?? text);
            finish(true, verb === "json" ? f.data.value : { text, truncated: f.data.truncated === true, modelTierApplied: f.data.model_tier_applied });
            return;
          case "error": {
            const code = String(f.data.code);
            const keep = code === "refused" ? undefined : code === "invalid_json" ? text || undefined : kept();
            finish(false, err(code, String(f.data.message), keep));
            return;
          }
          default:
            return;
        }
      });

      if (v.signal?.aborted) { finish(false, err("cancelled", "the signal was already aborted")); return; }
      v.signal?.addEventListener("abort", onAbort, { once: true });
      queueMicrotask(async () => {
        if (settled) return;
        let images: WireRequest["images"] = [];
        try { images = await prepareImages(v.images); } catch (e) { finish(false, e); return; }
        if (settled) return;
        started = true;
        rpc.call("sample", "run", [call, { ...v.req, images }]).catch((e: { code?: unknown; message?: unknown }) =>
          finish(false, err(String(e?.code ?? "upstream_error"), String(e?.message ?? e), kept())));
      });
    });
  }

  return {
    call: (input: unknown, options?: unknown) => run("text", input, options),
    json: (input: unknown, options?: unknown) => run("json", input, options),
    limits: () => rpc.call("sample", "limits", []),
  } as unknown as Record<string, (...args: never[]) => unknown>;
}

/** The members of the `sample` namespace besides the call itself (checked
 * against sample.d.ts in bridge/test/capabilities.test.ts). */
export const SAMPLE_METHODS = ["json", "limits"] as const;

/** The `sample` namespace `claude.use("sample")` resolves: a frozen function
 * (the call) carrying `json` and `limits`. */
export function sampleNamespace(rpc: Pick<Rpc, "call" | "on">): unknown {
  const m = makeSample(rpc) as Record<string, (...args: unknown[]) => unknown>;
  const ns = Object.assign((...args: unknown[]) => m.call(...args), { json: m.json, limits: m.limits });
  return Object.freeze(ns);
}
```

`web/bridge/src/parts/sample.ts`:

```ts
// The `sample` capability's page side: loaded on the first claude.use("sample")
// the shell grants, never with the other capabilities' members (parts/caps.ts).
export { SAMPLE_METHODS, sampleNamespace } from "../caps/sample";
```

`web/bridge/src/parts/types.ts`: `export type SamplePart = typeof import("./sample");` and `sample(attempt?: number): Promise<SamplePart>;` in `Parts`. `web/bridge/src/parts-url.ts`: `sample: load("sample")` in the returned object. `web/bridge/src/parts-static.ts`: `sample: loader<SamplePart>("sample", () => import("./parts/sample")),`. `web/scripts/build-parts.mjs` and `web/vite.bridge.config.ts`: the lists gain `"sample"`. `web/scripts/bundle-size.mjs`: `partKeys` gains `sample: "partSample"`.

`web/bridge/src/bridge.ts`: `parts` gains `sample: onNeed("sample", loaders.sample)`, and `locals` gains the arm (which narrows `name`, so the `as CapabilityName` cast and its import go):

```ts
    locals: (name, r, config) => name === "room"
      ? parts.room().then(p => p.roomNamespace(r))
      : name === "sample"
        ? parts.sample().then(p => p.sampleNamespace(r))
        : parts.caps().then(c => c.localsFor(name, r, config, { ctx: commentsContext, clip: clips })),
```

- [ ] **Step 4: Run the bridge tests to verify they pass**

Run: `cd web && npx vitest run bridge/test && npm run typecheck && npm run lint`
Expected: PASS.

- [ ] **Step 5: Measure, and add the part's budget**

Run: `cd web && npm run build && node scripts/bundle-size.mjs`
Expected: it fails only for the missing `partSample` budget. Add `"partSample": <floor(measured × 1.1)>` by hand and run it again.
Expected: PASS; the eager bridge within its budget, `caps` and `artifact` unchanged.

- [ ] **Step 6: Stage**

Stage the change (`git add web/bridge web/scripts web/vite.bridge.config.ts web/perf/bundle-budget.json`); do not commit. The controller commits it with the message:

```text
Load sample's page side as a lazy bridge part with its callable namespace, argument checks, streaming, page tools, and Stop
```

---

### Task 8: `sample` in the shell: the owner's browser only, consent, streaming, and the call count

The shell end of the sample relay. The handler and the SSE reader are carried over from the earlier draft of this plan, renamed, and changed for the owner's ruling and the scan: the shell offers `sample` only with the token (a LAN view's `use("sample")` resolves `null`); the daemon's sample status is fetched only when the page names `sample` (directly or through `permissions`), so no other capability waits on it (scan F8); every call carries the token; every refusal is mapped into `sample.d.ts`'s codes (F7); the handler is a lazy chunk (F14); it aborts its calls when the document leaves (B5). The count is a new component that touches the top bar in one line (scan B3; **align with Echo at merge**). The consent text and lifetime are open questions Q1 and Q2; the count's place and words are Q3 and Q5; this task builds the recommended answers.

**Files:**
- Create: `web/shell/src/sse.ts`, `web/shell/src/caps/sample.ts`, `web/shell/src/view/sample-count.ts`, `web/shell/src/ui/SampleCount.svelte`
- Modify: `web/shell/src/api.ts` (`SampleLimits`, `SampleStatus`, `getSampleStatus`), `web/shell/src/caps/availability.ts` (`Served`; `sample` for the owner when served; consent-gated; `CAPABILITIES` gains `sample`), `web/shell/src/caps/grants.ts` (`served`; the sample dialog; a view-only allow), `web/shell/src/caps/host.ts` (`CapEnv.{sampleStatus, onSampleCalls}`; the served check), `web/shell/src/caps/registry.ts` (`sample`), `web/shell/src/view/artifact-controller.ts` (`ViewState.sampleCalls`; the two `CapEnv` members), `web/shell/src/ui/TopbarIsland.svelte` (one line and its import)
- Modify: `web/e2e/fixtures.ts` (`startDaemon({config})`; a safe default `config.toml`)
- Create: `web/e2e/pages/sample.html`, `web/e2e/sample.spec.ts`
- Modify: `web/e2e/contract.spec.ts` (a case for `sample.html`)
- Test: `web/shell/src/sse.test.ts`, `web/shell/src/caps/sample.test.ts`, `web/shell/src/view/sample-count.test.ts` (new); `web/shell/src/caps/grants.test.ts`, `web/shell/src/caps/host.test.ts` (updated)

**Interfaces:**
- Consumes: Tasks 3–4's routes and frames; Task 6's `lazyHandler` and `Handler.leave`; Task 7's part; `Grants`, `consentGated`, `isAvailable`, `CapEnv`, `HandlerFactory`, `CapError`, `REGISTRY`, `promptQueue`, `Store`.
- Produces:
  - `type Served = { sample?: boolean }`; `isAvailable(name, declared, owner, served: Served = {})`; `new Grants(key, storage, declared, owner, ask, served: Served = {})`.
  - `CapEnv.sampleStatus?(): Promise<SampleStatus | null>` (fetched at most once per host, only with the token); `CapEnv.onSampleCalls?(n: number, cap: number | null): void`.
  - `sampleHandler: HandlerFactory`, `capErrorFrom(res: Response): Promise<CapError>`; `REGISTRY.sample` (lazy).
  - `readSse(body)`, `parseBlock(block)`; `type SampleStatus`, `type SampleLimits`, `getSampleStatus(id: string, token: string): Promise<SampleStatus | null>`.
  - `callCountText(n: number, cap: number | null): string`; `ViewState.sampleCalls: { n: number; cap: number | null } | null`.
  - E2E: `startDaemon(opts?: { config?: string })`, `NO_KEY_CONFIG`, `STUB_CONFIG`.

- [ ] **Step 1: Write the failing shell tests**

`web/shell/src/sse.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { parseBlock, readSse } from "./sse";

function stream(chunks: string[]): ReadableStream<Uint8Array> {
  const enc = new TextEncoder();
  return new ReadableStream({ start(c) { for (const s of chunks) c.enqueue(enc.encode(s)); c.close(); } });
}

describe("readSse", () => {
  it("yields whole events across chunk boundaries and skips comments", async () => {
    const got = [];
    for await (const e of readSse(stream(["event: start\nda", "ta: {\"a\":1}\n", "\n: keep-alive\n\nevent: text\r\ndata: x\r\n\r\n"]))) got.push(e);
    expect(got).toEqual([{ event: "start", data: "{\"a\":1}" }, { event: "text", data: "x" }]);
  });

  it("parses a block with the default event name", () => {
    expect(parseBlock("data: a\ndata: b")).toEqual({ event: "message", data: "a\nb" });
    expect(parseBlock(": only a comment")).toBeNull();
  });
});
```

`web/shell/src/caps/sample.test.ts`:

```ts
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { Grants, type PromptAnswer } from "./grants";
import type { CapEnv } from "./host";
import { sampleHandler } from "./sample";

class MemoryStorage { data = new Map<string, string>(); getItem(k: string) { return this.data.get(k) ?? null; } setItem(k: string, v: string) { this.data.set(k, v); } }

function sseBody(frames: [string, unknown][]) {
  return frames.map(([e, d]) => `event: ${e}\ndata: ${JSON.stringify(d)}\n\n`).join("");
}

function setup(answer: PromptAnswer = "allow") {
  const posted: ShellToBridge[] = [];
  const counts: number[] = [];
  const ask = vi.fn(async () => answer);
  const status = { available: true, provider: "stub", limits: { maxPromptBytes: 65536, tools: { maxCount: 16 } }, calls_today: 0, daily_call_cap: null };
  const env = { aid: "7q3k9mzx2b4t", token: "tk", post: (m: ShellToBridge) => posted.push(m), sampleStatus: async () => status, onSampleCalls: (n: number) => counts.push(n) } as unknown as CapEnv;
  const grants = new Grants("k", new MemoryStorage() as unknown as Storage, { sample: {} }, true, ask, { sample: true });
  const handler = sampleHandler(env, grants);
  const frames = () => posted.filter(m => m.type === "clax:event").map(m => (m as { data: { call: string; event: string; data: unknown } }).data);
  return { handler, ask, frames, counts };
}

afterEach(() => { vi.unstubAllGlobals(); });

describe("sample handler", () => {
  it("asks once, posts the call, relays its frames, and reports the count", async () => {
    const fetchMock = vi.fn(async () => new Response(sseBody([["start", { call_id: "c1", cached: false, calls_today: 3, daily_call_cap: null }], ["text", { delta: "hi" }], ["done", { text: "hi", truncated: false, model_tier_applied: "default" }]]), { headers: { "content-type": "text/event-stream" } }));
    vi.stubGlobal("fetch", fetchMock);
    const { handler, ask, frames, counts } = setup();
    await handler.call("run", ["s1", { input: "q" }]);
    await handler.call("run", ["s2", { input: "q" }]);
    expect(ask).toHaveBeenCalledTimes(1);
    expect(ask.mock.calls[0][0]).toMatchObject({ body: expect.stringContaining("Anthropic API key") });
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/artifacts/7q3k9mzx2b4t/sample");
    expect(new Headers(init.headers).get("authorization")).toBe("Bearer tk");
    expect(JSON.parse(String(init.body))).toEqual({ input: "q" });
    expect(frames().filter(f => f.call === "s1").map(f => f.event)).toEqual(["start", "text", "done"]);
    expect(counts).toEqual([3, 3]);
  });

  it("a declined viewer gets not_granted and nothing is sent", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const { handler } = setup("deny");
    await expect(handler.call("run", ["s1", { input: "q" }])).rejects.toMatchObject({ code: "not_granted" });
    await expect(handler.call("run", ["s2", { input: "q" }])).rejects.toMatchObject({ code: "not_granted" });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("a refusal before the stream rejects with the daemon's code", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { code: "rate_limited", message: "cap" } }), { status: 429 })));
    const { handler } = setup();
    await expect(handler.call("run", ["s1", { input: "q" }])).rejects.toMatchObject({ code: "rate_limited", message: "cap" });
  });

  it("a stream that stops without done or error ends upstream_error", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(sseBody([["start", { call_id: "c", cached: false, calls_today: 1 }], ["text", { delta: "par" }]]))));
    const { handler, frames } = setup();
    await handler.call("run", ["s1", { input: "q" }]);
    expect(frames().at(-1)).toMatchObject({ call: "s1", event: "error", data: { code: "upstream_error" } });
  });

  it("cancel aborts the request; a call cancelled during consent is never sent", async () => {
    let seen: AbortSignal | undefined;
    vi.stubGlobal("fetch", vi.fn((_u: string, init: RequestInit) => { seen = init.signal ?? undefined; return new Promise(() => {}); }));
    const { handler } = setup();
    void handler.call("run", ["s1", { input: "q" }]);
    await vi.waitFor(() => expect(seen).toBeDefined());
    await handler.call("cancel", ["s1"]);
    expect(seen!.aborted).toBe(true);

    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    let answer!: (a: PromptAnswer) => void;
    const posted: ShellToBridge[] = [];
    const grants = new Grants("k2", null, { sample: {} }, true, () => new Promise(r => { answer = r; }), { sample: true });
    const h = sampleHandler({ aid: "7q3k9mzx2b4t", token: "tk", post: (m: ShellToBridge) => posted.push(m) } as unknown as CapEnv, grants);
    const p = h.call("run", ["s9", { input: "q" }]);
    await vi.waitFor(() => expect(answer).toBeTypeOf("function"));
    await h.call("cancel", ["s9"]);
    answer("allow");
    await expect(p).resolves.toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("tool results go to the daemon", async () => {
    const fetchMock = vi.fn(async () => new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchMock);
    const { handler } = setup();
    await handler.call("toolResult", ["01JCALL", "toolu_1", "teal", false]);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/artifacts/7q3k9mzx2b4t/sample/01JCALL/tool_result");
    expect(JSON.parse(String(init.body))).toEqual({ id: "toolu_1", content: "teal", is_error: false });
  });

  it("limits come from the status the shell fetched", async () => {
    const { handler } = setup();
    await expect(handler.call("limits", [])).resolves.toEqual({ maxPromptBytes: 65536, tools: { maxCount: 16 } });
  });
  it("tool results use the daemon's call ID from the start frame", async () => {
    const fetchMock = vi.fn(async (url: string) => (url.endsWith("/sample")
      ? new Response(sseBody([["start", { call_id: "01JDAEMON", cached: false, calls_today: 1 }], ["tool_call", { id: "toolu_1", name: "t", input: {} }]]))
      : new Response(null, { status: 204 })));
    vi.stubGlobal("fetch", fetchMock);
    const { handler } = setup();
    await handler.call("run", ["s1", { input: "q" }]);
    await handler.call("toolResult", ["s1", "toolu_1", "teal", false]);
    expect(fetchMock.mock.calls.at(-1)![0]).toBe("/api/artifacts/7q3k9mzx2b4t/sample/01JDAEMON/tool_result");
  });

  it("maps the daemon's refusals into the contract's codes", async () => {
    const { handler } = setup();
    for (const [status, code, want] of [
      [401, "unauthorized", "session_expired"],
      [403, "forbidden", "upstream_error"],
      [403, "forbidden_origin", "upstream_error"],
      [404, "not_found", "not_declared"],
      [408, "timeout", "upstream_error"],
      [413, "payload_too_large", "prompt_too_large"],
      [403, "not_declared", "not_declared"],
      [429, "rate_limited", "rate_limited"],
    ] as const) {
      vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { code, message: "m" } }), { status })));
      await expect(handler.call("run", [`s${status}${code}`, { input: "q" }]), `${status} ${code}`).rejects.toMatchObject({ code: want });
    }
  });

  it("leave aborts every call in flight", async () => {
    const signals: AbortSignal[] = [];
    vi.stubGlobal("fetch", vi.fn((_u: string, init: RequestInit) => { signals.push(init.signal!); return new Promise(() => {}); }));
    const { handler } = setup();
    void handler.call("run", ["s1", { input: "a" }]);
    void handler.call("run", ["s2", { input: "b" }]);
    await vi.waitFor(() => expect(signals).toHaveLength(2));
    handler.leave!();
    expect(signals.every(s => s.aborted)).toBe(true);
  });
});
```

`web/shell/src/view/sample-count.test.ts`:

```ts
import { expect, it } from "vitest";
import { callCountText } from "./sample-count";

it("counts calls today, against the cap when there is one", () => {
  expect(callCountText(0, null)).toBe("0 Claude calls today");
  expect(callCountText(1, null)).toBe("1 Claude call today");
  expect(callCountText(3, 200)).toBe("3 of 200 Claude calls today");
});
```

In `web/shell/src/caps/grants.test.ts`, add:

```ts
  it("sample: the owner's browser only, when served; asked in its own dialog; allowed for this view only", async () => {
    const storage = new MemoryStorage();
    const ask = vi.fn(async () => "allow" as const);
    const key = grantsKey("7q3k9mzx2b4t", "u_00000000000000000000aa");
    const g = new Grants(key, storage as unknown as Storage, { sample: {} }, true, ask, { sample: true });
    expect(g.state("sample")).toBe("prompt");
    await g.request(["sample"]);
    expect(ask.mock.calls[0][0]).toMatchObject({ title: "Let this page ask Claude?", body: expect.stringContaining("Anthropic API key") });
    expect(g.state("sample")).toBe("granted");
    // A new view asks again: the allow was never stored.
    expect(new Grants(key, storage as unknown as Storage, { sample: {} }, true, ask, { sample: true }).state("sample")).toBe("prompt");
    expect(new Grants(key, storage as unknown as Storage, { sample: {} }, true, ask).state("sample")).toBe("unavailable");
    expect(new Grants(key, storage as unknown as Storage, { sample: {} }, false, ask, { sample: true }).state("sample")).toBe("unavailable");
  });
```

In `web/shell/src/caps/host.test.ts`, add:

```ts
  it("asks the daemon about sample only when the page names it, and offers it only to the owner's browser", async () => {
    const status = { available: true, provider: "stub", limits: { maxPromptBytes: 65536 }, calls_today: 4, daily_call_cap: null };
    const sampleStatus = vi.fn(async () => status);
    const counts: number[] = [];
    const { e, posted } = env({ declared: { db: {}, sample: {} }, sampleStatus, onSampleCalls: n => { counts.push(n); } });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    await host.handle({ type: "clax:use", id: "u1", name: "db" });
    expect(sampleStatus).not.toHaveBeenCalled();
    await host.handle({ type: "clax:use", id: "u2", name: "sample" });
    await host.handle({ type: "clax:use", id: "u3", name: "sample" });
    expect(sampleStatus).toHaveBeenCalledTimes(1);
    expect(posted.map(m => (m as { granted: boolean }).granted)).toEqual([true, true, true]);
    expect(counts).toEqual([4]);

    const lan = env({ declared: { sample: {} }, token: null, sampleStatus });
    await new CapabilityHost(Promise.resolve(lan.e), REGISTRY, null).handle({ type: "clax:use", id: "u", name: "sample" });
    expect(lan.posted[0]).toMatchObject({ granted: false });
    expect(sampleStatus).toHaveBeenCalledTimes(1);

    const off = env({ declared: { sample: {} }, sampleStatus: async () => ({ ...status, available: false }) });
    await new CapabilityHost(Promise.resolve(off.e), REGISTRY, null).handle({ type: "clax:use", id: "u", name: "sample" });
    expect(off.posted[0]).toMatchObject({ granted: false });
  });
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && npx vitest run shell/src`
Expected: FAIL (`./sse`, `./sample`, `./sample-count` do not exist; `Grants` takes no `served`; `sample` is never granted).

- [ ] **Step 3: Implement availability, consent, and the served check**

`web/shell/src/caps/availability.ts`:

```ts
/** What this daemon serves beyond the declaration: `sample` needs a provider
 * (`GET /api/artifacts/<aid>/sample`), asked when the page names it. */
export type Served = { sample?: boolean };

export const CAPABILITIES = ["artifact", "db", "downloads", "user", "comments", "assets", "room", "sample"] as const;
```

`isAvailable(name: string, declared: Declared, owner: boolean, served: Served = {}): boolean` gains `case "sample": return owner && declares(name, declared) && served.sample === true;` (the owner's browser only: owner ruling), and the header comment's last clause becomes "`room` when declared; `sample` to the owner shell, when declared and the daemon has a provider; files and mcp never". `consentGated` becomes:

```ts
export function consentGated(name: string, declared: Declared): boolean {
  return name === "sample" || (name === "comments" && declared.comments?.composer_only !== true);
}
```

`web/shell/src/caps/grants.ts`: the header comment's second sentence becomes "Grants persist in localStorage under `clax.grants.v1:<aid>:<viewer public ID>`, except `sample`'s, which lasts for the view (sample.d.ts: the first call in a view asks); a denial or a dismissed prompt lasts for the page load only." Then:

```ts
const ASKS: Record<string, string> = {
  comments: "post comments on this artifact under your name",
  sample: "send requests to Claude, paid for by this machine's Anthropic API key",
};

/** The dialog of a capability asked on its own (open question Q2). */
const ALONE: Record<string, Prompt> = {
  sample: {
    title: "Let this page ask Claude?",
    body: "Each request this page sends to Claude is paid for by this machine's Anthropic API key. Clax asks again the next time this page loads.",
    allow: "Allow",
    deny: "Don't allow",
  },
};

/** Capabilities whose "Allow" lasts for this view only (open question Q1). */
const VIEW_ONLY = new Set(["sample"]);
```

The constructor takes a sixth parameter `private readonly served: Served = {}` (`import { …, type Served } from "./availability"`), `state()` calls `isAvailable(name, this.declared, this.owner, this.served)`, and in `request()`:

```ts
      const alone = askable.length === 1 ? ALONE[askable[0]] : undefined;
      const answer = await this.ask(alone ?? {
        title: "Allow this page to act as you?",
        body: `This page asks to ${askable.map(n => ASKS[n] ?? `use ${n}`).join(" and ")}.`,
        allow: "Allow",
        deny: "Don't allow",
      });
      // …the loop over askable is unchanged…
      if (answer === "allow") write(this.storage, this.key, [...this.granted].filter(n => !VIEW_ONLY.has(n)));
```

`web/shell/src/api.ts`:

```ts
export type SampleLimits = { maxPromptBytes: number; images?: { maxCount: number; maxInputBytes: number; mediaTypes: string[] }; tools?: { maxCount: number } };
/** `GET /api/artifacts/<id>/sample` with the token: whether this daemon samples for the artifact, and today's count. */
export type SampleStatus = { available: boolean; provider: string | null; limits: SampleLimits; calls_today: number; daily_call_cap: number | null };

/** The artifact's sample status as the owner's browser sees it, or null when it cannot be read. */
export async function getSampleStatus(id: string, token: string): Promise<SampleStatus | null> {
  try {
    const res = await fetch(`/api/artifacts/${id}/sample`, { headers: { authorization: `Bearer ${token}` } });
    return res.ok ? ((await res.json()) as SampleStatus) : null;
  } catch {
    return null;
  }
}
```

`web/shell/src/caps/host.ts`: `CapEnv` gains

```ts
  /** `GET /api/artifacts/<aid>/sample` with the token: set only for the owner
   * shell; called at most once per host, and only once the page names `sample`. */
  sampleStatus?(): Promise<SampleStatus | null>;
  /** The artifact's calls to Claude today, and the cap: the top bar's count. */
  onSampleCalls?(n: number, cap: number | null): void;
```

and `CapabilityHost` gains a field and a method, and uses them:

```ts
  /** What the daemon serves beyond the declaration; filled by `checkServed`. */
  private readonly served: Served = {};
  private servedChecked: Promise<void> | null = null;

  /** Asks the daemon once whether it samples for this view, only for the
   * owner shell of an artifact that declares `sample`. */
  private checkServed(env: CapEnv): Promise<void> {
    if (env.token === null || !env.sampleStatus || !Object.hasOwn(env.declared, "sample")) return Promise.resolve();
    return this.servedChecked ??= env.sampleStatus().then(s => {
      this.served.sample = s?.available === true;
      if (s?.available) env.onSampleCalls?.(s.calls_today, s.daily_call_cap);
    }, () => {});
  }
```

In the constructor, `new Grants(…, e.prompt, this.served)`. In `handle`, after `if (this.dead) return;`:

```ts
    const named = m.type === "clax:use" ? m.name : m.ns;
    // Only a request about sample (or permissions, which lists it) waits for the daemon's answer.
    if (named === "sample" || named === "permissions") await this.checkServed(env);
    if (this.dead) return;
```

and both `isAvailable` calls pass `this.served`. `web/shell/src/caps/registry.ts`: `sample: lazyHandler(() => import("./sample").then(m => m.sampleHandler)),`.

- [ ] **Step 4: Implement the SSE reader, the handler, and the count**

`web/shell/src/sse.ts`:

```ts
// Reads a `text/event-stream` response body (the sample route's), yielding one
// {event, data} per event; comments (keep-alives) are skipped.

export type SseMessage = { event: string; data: string };

export function parseBlock(block: string): SseMessage | null {
  let event = "message";
  const data: string[] = [];
  for (const line of block.split("\n")) {
    if (line.startsWith(":")) continue;
    if (line.startsWith("event:")) event = line.slice(6).trim();
    else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
  }
  return data.length ? { event, data: data.join("\n") } : null;
}

export async function* readSse(body: ReadableStream<Uint8Array>): AsyncGenerator<SseMessage> {
  const reader = body.getReader();
  const dec = new TextDecoder();
  let buf = "";
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      buf = (buf + dec.decode(value, { stream: true })).replace(/\r\n/g, "\n");
      let i: number;
      while ((i = buf.indexOf("\n\n")) >= 0) {
        const m = parseBlock(buf.slice(0, i));
        buf = buf.slice(i + 2);
        if (m) yield m;
      }
    }
  } finally {
    reader.releaseLock();
  }
}
```

`web/shell/src/caps/sample.ts`:

```ts
// The `sample` capability's shell side, in the owner's browser only (the
// host offers `sample` only with the token): consent before the first call of
// the view (Grants: one dialog; open question Q1), the call over
// `POST /api/artifacts/<aid>/sample` with the bearer token, each SSE frame
// relayed to the page as `clax:event {ns: "sample", topic: "frame", data:
// {call, event, data}}`, cancellation, page tool results, and the top bar's
// call count. Every refusal reaches the page as one of sample.d.ts's codes.
// This module is a lazy chunk (registry.ts), loaded on the page's first call.
import { readSse } from "../sse";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

/** sample.d.ts's `SampleErrorCode`: the only codes a page may see. */
const SAMPLE_CODES = new Set(["invalid_request", "prompt_too_large", "images_unavailable", "tools_unavailable", "image_rejected", "cancelled", "not_granted", "session_expired", "sampling_disabled", "not_declared", "rate_limited", "refused", "empty_completion", "invalid_json", "upstream_error", "capability_disabled", "capability_removed", "transform_error", "queue_overflow"]);

/** A refusal before the stream, as the page may see it: a token the daemon
 * no longer takes (it restarted) is `session_expired`, a missing artifact
 * `not_declared`, a body over the limit `prompt_too_large`, and any code
 * outside the contract (`forbidden`, `forbidden_origin`, `timeout`) `upstream_error`. */
export async function capErrorFrom(res: Response): Promise<CapError> {
  let code = "";
  let message = `HTTP ${res.status}`;
  try {
    const e = (await res.json()).error as { code?: unknown; message?: unknown };
    code = String(e?.code ?? "");
    if (typeof e?.message === "string") message = e.message;
  } catch { /* not JSON */ }
  if (res.status === 401) return new CapError("session_expired", "the page's session with Clax ended; reload the page");
  if (res.status === 404) return new CapError("not_declared", message);
  if (res.status === 413) return new CapError("prompt_too_large", message);
  return new CapError(SAMPLE_CODES.has(code) ? code : "upstream_error", message);
}

export const sampleHandler: HandlerFactory = (env, grants) => {
  const controllers = new Map<string, AbortController>();
  const cancelled = new Set<string>();
  const daemonIds = new Map<string, string>();
  const auth = { authorization: `Bearer ${env.token ?? ""}` };
  const frame = (call: string, event: string, data: unknown) =>
    env.post({ type: "clax:event", ns: "sample", topic: "frame", data: { call, event, data } });

  async function run(call: string, req: unknown): Promise<null> {
    if (grants.state("sample") === "prompt") await grants.request(["sample"]);
    if (cancelled.has(call)) { cancelled.delete(call); return null; }
    if (grants.state("sample") !== "granted") throw new CapError("not_granted", "the viewer has not allowed this page to use Claude");
    const ctl = new AbortController();
    controllers.set(call, ctl);
    try {
      let res: Response;
      try {
        res = await fetch(`/api/artifacts/${env.aid}/sample`, { method: "POST", headers: { "content-type": "application/json", ...auth }, body: JSON.stringify(req), signal: ctl.signal });
      } catch (e) {
        if (ctl.signal.aborted) return null;
        throw new CapError("upstream_error", e instanceof Error ? e.message : String(e));
      }
      if (!res.ok || !res.body) throw await capErrorFrom(res);
      try {
        for await (const ev of readSse(res.body)) {
          let data: Record<string, unknown>;
          try { data = JSON.parse(ev.data) as Record<string, unknown>; } catch { continue; }
          if (ev.event === "start") {
            if (typeof data.call_id === "string") daemonIds.set(call, data.call_id);
            if (typeof data.calls_today === "number") env.onSampleCalls?.(data.calls_today, typeof data.daily_call_cap === "number" ? data.daily_call_cap : null);
          }
          frame(call, ev.event, data);
          if (ev.event === "done" || ev.event === "error") return null;
        }
        if (!ctl.signal.aborted) frame(call, "error", { code: "upstream_error", message: "the answer stopped before it finished" });
      } catch (e) {
        if (!ctl.signal.aborted) frame(call, "error", { code: "upstream_error", message: e instanceof Error ? e.message : String(e) });
      }
      return null;
    } finally {
      controllers.delete(call);
    }
  }

  /** The page went away: every call stops, and the daemon drops the provider request. */
  function end() {
    for (const c of controllers.values()) c.abort();
    controllers.clear();
    daemonIds.clear();
    cancelled.clear();
  }

  return {
    async call(method, args) {
      switch (method) {
        case "run":
          return run(String(args[0]), args[1]);
        case "cancel": {
          const call = String(args[0]);
          const ctl = controllers.get(call);
          if (ctl) ctl.abort(); else cancelled.add(call);
          daemonIds.delete(call);
          return null;
        }
        case "toolResult": {
          const [call, id, content, isError] = args;
          const daemonId = daemonIds.get(String(call)) ?? String(call);
          const res = await fetch(`/api/artifacts/${env.aid}/sample/${encodeURIComponent(daemonId)}/tool_result`, {
            method: "POST", headers: { "content-type": "application/json", ...auth },
            body: JSON.stringify({ id, content, is_error: isError === true }),
          });
          if (!res.ok && res.status !== 404) throw await capErrorFrom(res);
          return null;
        }
        case "limits":
          return (await env.sampleStatus?.())?.limits ?? { maxPromptBytes: 65536 };
        default:
          throw new CapError("capability_removed", `sample.${method} is not part of this runtime`);
      }
    },
    reset: end,
    leave: end,
    dispose: end,
  };
};
```

`web/shell/src/view/sample-count.ts` (the words are open question Q5):

```ts
/** The top bar's running count of this artifact's calls to Claude today. */
export function callCountText(n: number, cap: number | null): string {
  return cap === null ? `${n} Claude call${n === 1 ? "" : "s"} today` : `${n} of ${cap} Claude calls today`;
}
```

`web/shell/src/ui/SampleCount.svelte` (its place is open question Q3; **align with Echo at merge**):

```svelte
<script lang="ts">
  // The running count of this artifact's calls to Claude today on this
  // machine's API key (spec §14), in the owner's browser once the page has
  // asked for sample and the daemon has a provider. Hidden at phone width.
  import { fromStore } from "svelte/store";
  import type { ArtifactController } from "../view/artifact-controller";
  import { callCountText } from "../view/sample-count";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const calls = $derived(view.current.sampleCalls);
</script>

{#if calls}
  <span class="sample-count hide-sm" title="Calls this artifact made to Claude today with this machine's API key">{callCountText(calls.n, calls.cap)}</span>
{/if}

<style>
  .sample-count { font-size: 13px; color: var(--muted); white-space: nowrap; }
</style>
```

`web/shell/src/ui/TopbarIsland.svelte`: `import SampleCount from "./SampleCount.svelte";` and, as the first child inside `{#if viewReady(s)}` (before the Comment button; on Echo's bar, after the `who-slot`), `<SampleCount {ctl} />`. A `span` before the buttons leaves the bar's `button:first-of-type` and `select ~ .hide-sm` styling as it is.

`web/shell/src/view/artifact-controller.ts`: `ViewState` gains

```ts
  /** This artifact's calls to Claude today and the cap, once the owner's
   * browser has asked the daemon (null until then, and on a LAN view). */
  sampleCalls: { n: number; cap: number | null } | null;
```

initialised `sampleCalls: null` in the constructor's state. In `viewChanged()`, the host's environment becomes `getToken().then(token => { let status: Promise<SampleStatus | null> | null = null; return { …every current field unchanged…, sampleStatus: token === null ? undefined : () => status ??= getSampleStatus(this.id, token), onSampleCalls: (n, cap) => this.set({ sampleCalls: { n, cap } }) }; })` (import `getSampleStatus` and `type SampleStatus` from `../api`).

- [ ] **Step 5: Run the shell tests to verify they pass**

Run: `cd web && npm test -- --reporter=dot && npm run typecheck && npm run lint`
Expected: PASS, `topbar-style.test.ts` included.

- [ ] **Step 6: Make every e2e daemon safe, and write the sample page and the failing browser tests**

In `web/e2e/fixtures.ts`, give `startDaemon` a config and a safe default (no Playwright daemon may reach a real provider); `mkdtempSync`, the `--port 0` and the rest of the function stay:

```ts
import { writeFileSync } from "node:fs";

/** The default config.toml: the sample key comes from a variable nobody sets, so no test reaches a real provider. */
export const NO_KEY_CONFIG = '[sample]\napi_key_env = "CLAX_E2E_UNSET_KEY"\n';
/** The stub provider: canned, deterministic answers (see crates/clax-server/src/sample/stub.rs). */
export const STUB_CONFIG = '[sample]\nprovider = "stub"\nstub_delay_ms = 150\n';

export async function startDaemon(opts: { config?: string } = {}) {
  const home = mkdtempSync(join(tmpdir(), "clax-e2e-"));
  writeFileSync(join(home, "config.toml"), opts.config ?? NO_KEY_CONFIG);
  const child: ChildProcess = spawn("cargo", ["run", "-q", "-p", "clax-cli", "--", "serve", "--foreground", "--bind", "127.0.0.1", "--port", "0"],
    { cwd: repoRoot, env: { ...process.env, CLAX_HOME: home, CLAX_CODEX_BIN: "", CLAX_E2E_UNSET_KEY: "" }, stdio: ["ignore", "inherit", "inherit"] });
  // … the rest of startDaemon unchanged
```

`web/e2e/pages/sample.html`:

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Ask about this page</title>
<style>
:root { --bg: #ffffff; --fg: #1d1d1f; --muted: #6e6e73; --line: #d2d2d7; --accent: #0a66c2; }
@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { --bg: #161618; --fg: #f5f5f7; --muted: #a1a1a6; --line: #3a3a3c; --accent: #4ea1ff; } }
:root[data-theme="dark"] { --bg: #161618; --fg: #f5f5f7; --muted: #a1a1a6; --line: #3a3a3c; --accent: #4ea1ff; }
body { margin: 0; padding: 16px; background: var(--bg); color: var(--fg); font: 15px/1.5 system-ui, sans-serif; }
textarea { width: 100%; box-sizing: border-box; min-height: 4em; font: inherit; color: var(--fg); background: transparent; border: 1px solid var(--line); border-radius: 6px; padding: 8px; }
button { font: inherit; padding: 6px 12px; border: 1px solid var(--line); border-radius: 6px; background: transparent; color: var(--fg); cursor: pointer; }
button:hover { border-color: var(--accent); }
pre { white-space: pre-wrap; overflow-wrap: anywhere; }
.muted { color: var(--muted); }
</style>
</head>
<body>
<h1>Ask about this page</h1>
<p id="status" class="muted">loading</p>
<textarea id="prompt" aria-label="Prompt">What is a haiku?</textarea>
<p><button id="ask">Ask</button> <button id="stop">Stop</button> <button id="tool">Ask with a tool</button></p>
<pre id="out"></pre>
<p class="muted">updates <span id="updates">0</span> · tool runs <span id="tool-runs">0</span> · <span id="note"></span></p>
<pre id="kept"></pre>
<pre id="limits" class="muted"></pre>
<script>
(async () => {
  const sample = await claude.use("sample");
  const status = document.getElementById("status");
  if (!sample) {
    status.textContent = "unavailable";
    for (const id of ["ask", "stop", "tool"]) document.getElementById(id).hidden = true;
    return;
  }
  status.textContent = "ready";
  document.getElementById("limits").textContent = JSON.stringify(await sample.limits().catch(() => null));
  const out = document.getElementById("out"), note = document.getElementById("note"), kept = document.getElementById("kept"), updates = document.getElementById("updates");
  let ctl = null, runs = 0;
  document.getElementById("stop").onclick = () => ctl?.abort();
  async function ask(options) {
    ctl = new AbortController();
    let n = 0;
    updates.textContent = "0"; note.textContent = ""; kept.textContent = ""; out.textContent = "Thinking...";
    try {
      const { text } = await sample(document.getElementById("prompt").value, {
        signal: ctl.signal,
        onText: ({ text }) => { out.textContent = text; updates.textContent = String(++n); },
        ...options,
      });
      out.textContent = text;
      note.textContent = "done";
    } catch (e) {
      out.textContent = "";
      note.textContent = e.code;
      kept.textContent = e.text ?? "";
    }
  }
  document.getElementById("ask").onclick = () => ask({});
  document.getElementById("tool").onclick = () => ask({
    tools: [{ name: "getColor", description: "Returns the page's accent colour name.", execute: () => { document.getElementById("tool-runs").textContent = String(++runs); return "teal"; } }],
  });
})();
</script>
</body>
</html>
```

`web/e2e/sample.spec.ts`:

```ts
import { readFileSync } from "node:fs";
import { test, expect, type Frame, type Page } from "@playwright/test";
import { STUB_CONFIG, contentFrame, openArtifact, publishWith, startDaemon } from "./fixtures";

const PAGE = readFileSync(new URL("./pages/sample.html", import.meta.url), "utf8");

test.describe("with the stub provider", () => {
  let d: Awaited<ReturnType<typeof startDaemon>>;
  test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon({ config: STUB_CONFIG }); });
  test.afterAll(async () => { await d?.stop(); });

  async function open(page: Page, title: string, mode: "subdomain" | "sandbox"): Promise<Frame> {
    const { artifact } = await publishWith(d.base, d.token, title, PAGE, { sample: {} });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(f.locator("#status")).toHaveText("ready");
    return f;
  }

  /** Fills the prompt, clicks `button`, and answers the consent dialog with `consent` (every test's first call asks). */
  async function ask(page: Page, f: Frame, prompt: string, button = "#ask", consent: "Allow" | "Don't allow" = "Allow") {
    await f.locator("#prompt").fill(prompt);
    await f.locator(button).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("Anthropic API key");
    await dialog.getByRole("button", { name: consent, exact: true }).click();
  }

  for (const mode of ["subdomain", "sandbox"] as const) {
    test(`${mode}: consent once, progressive text, and the call count`, async ({ page }) => {
      const f = await open(page, `Sample ${mode}`, mode);
      await expect(page.locator(".sample-count")).toHaveText("0 Claude calls today");
      await ask(page, f, "tell me about three bears");
      await expect(f.locator("#note")).toHaveText("done");
      await expect(f.locator("#out")).toHaveText("echo: tell me about three bears");
      expect(Number(await f.locator("#updates").textContent())).toBeGreaterThan(1);
      await expect(page.locator(".sample-count")).toHaveText("1 Claude call today");
      await f.locator("#prompt").fill("another question");
      await f.locator("#ask").click();
      await expect(f.locator("#note")).toHaveText("done");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(page.locator(".sample-count")).toHaveText("2 Claude calls today");
      expect(JSON.parse(await f.locator("#limits").textContent() ?? "null")).toEqual({ maxPromptBytes: 65536, tools: { maxCount: 16 } });
    });

    test(`${mode}: consent lasts for the view: a reload asks again`, async ({ page }) => {
      const { artifact } = await publishWith(d.base, d.token, `Sample reload ${mode}`, PAGE, { sample: {} });
      const f = await openArtifact(page, d.base, artifact.id, 1, mode);
      await expect(f.locator("#status")).toHaveText("ready");
      await ask(page, f, "first");
      await expect(f.locator("#note")).toHaveText("done");
      await page.reload();
      const again = await contentFrame(page, artifact.id, 1);
      await expect(again.locator("#status")).toHaveText("ready");
      await ask(page, again, "second");
      await expect(again.locator("#note")).toHaveText("done");
    });

    test(`${mode}: a declined viewer gets not_granted without asking again`, async ({ page }) => {
      const f = await open(page, `Sample deny ${mode}`, mode);
      await ask(page, f, "q", "#ask", "Don't allow");
      await expect(f.locator("#note")).toHaveText("not_granted");
      await f.locator("#ask").click();
      await expect(f.locator("#note")).toHaveText("not_granted");
      await expect(page.getByRole("dialog")).toHaveCount(0);
    });

    test(`${mode}: a page tool runs and its result reaches the answer`, async ({ page }) => {
      const f = await open(page, `Sample tool ${mode}`, mode);
      await ask(page, f, "[[tool:getColor]]", "#tool");
      await expect(f.locator("#note")).toHaveText("done");
      await expect(f.locator("#tool-runs")).toHaveText("1");
      await expect(f.locator("#out")).toHaveText("Checking.\n\ntool said: teal");
    });

    test(`${mode}: Stop rejects cancelled and keeps the partial text`, async ({ page }) => {
      const f = await open(page, `Sample stop ${mode}`, mode);
      await ask(page, f, "[[slow]]");
      await expect(f.locator("#out")).toContainText("tick");
      await f.locator("#stop").click();
      await expect(f.locator("#note")).toHaveText("cancelled");
      await expect(f.locator("#kept")).toContainText("tick");
    });
  }
});

test.describe("without a key", () => {
  let d: Awaited<ReturnType<typeof startDaemon>>;
  test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
  test.afterAll(async () => { await d?.stop(); });

  for (const mode of ["subdomain", "sandbox"] as const) {
    test(`${mode}: use("sample") resolves null and the header shows no count`, async ({ page }) => {
      const { artifact } = await publishWith(d.base, d.token, `No key ${mode}`, PAGE, { sample: {} });
      const f = await openArtifact(page, d.base, artifact.id, 1, mode);
      await expect(f.locator("#status")).toHaveText("unavailable");
      await expect(page.locator(".sample-count")).toHaveCount(0);
    });
  }
});

test.describe("a LAN viewer, with the stub provider", () => {
  let d: Awaited<ReturnType<typeof startDaemon>>;
  test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon({ config: STUB_CONFIG }); });
  test.afterAll(async () => { await d?.stop(); });

  test("use(\"sample\") resolves null: no dialog, no count, nothing spent", async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, "Sample LAN", PAGE, { sample: {} });
    const asked: string[] = [];
    page.on("request", r => { if (r.url().includes("/sample")) asked.push(r.url()); });
    const f = await openArtifact(page, d.base, artifact.id, 1, "sandbox", { lan: true });
    await expect(f.locator("#status")).toHaveText("unavailable");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.locator(".sample-count")).toHaveCount(0);
    expect(asked).toEqual([]);
  });
});
```

In `web/e2e/contract.spec.ts`, add to `CASES` (its daemon is `startDaemon()`, which has no key):

```ts
  "sample.html": {
    caps: { sample: {} },
    async check(f) {
      await expect(f.locator("#status")).toHaveText("unavailable");
      await expect(f.locator("#ask")).toBeHidden();
    },
  },
```

- [ ] **Step 7: Run the browser tests and the time-to-usable gates**

Run: `cd web && npm run build && npx playwright test sample.spec.ts room.spec.ts contract.spec.ts capabilities.spec.ts && node scripts/bundle-size.mjs && npm run perf`
Expected: PASS in both modes; `artifact` in `bundle-size.mjs` grows only by `SampleCount` and the controller's lines (the handler is a dynamic chunk) and stays within its budget; `perf` within its budgets. If `artifact` is over its budget, report it to the controller rather than raising it (open question Q3's alternative: a lazily mounted count).

Then by hand: `H=$(mktemp -d); printf '[sample]\nprovider = "stub"\nstub_delay_ms = 150\n' > $H/config.toml; CLAX_HOME=$H CLAX_CODEX_BIN= cargo run -p clax-cli -- serve --foreground --bind 127.0.0.1 --port 0`, publish `sample.html` with `{"sample": {}}` (token and port from `$H/daemon.json`), and check the consent dialog's words and its 500 ms Allow arming, the streaming text, Stop, and the top bar's count in light mode, dark mode and at phone width (the count hidden there; the bar's other controls never pushed off screen). Stop the daemon and remove `$H`.

- [ ] **Step 8: Stage**

Stage the change (`git add web/shell web/e2e`); do not commit. The controller commits it with the message:

```text
Serve sample to the owner's browser with consent per view, streaming, page tools, Stop, the contract's error codes, and a running call count
```

---

### Task 9: The spec, the contract in the docs and skills, the scripted smoke, and the demo

Written fresh: the documentation has been reworded since the earlier draft (scan F10), the smoke follows `scripts/smoke-capabilities.sh`'s conventions (F11), and the demo runs its own scratch daemon, never the dev home (F12 and Global Constraints). Grok Build rewrites parts of the spec and `docs/contract.md`, and may add a fourth copy of the shared skill section: edit the files as they are on `main` when this task runs, find each place by its heading and words (not by line number), and make every copy that `scripts/test-plugins.sh` compares identical.

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-clax-design.md`, `docs/contract.md`, `plugins/claude-code/skills/clax/SKILL.md`, `plugins/clax/skills/clax/SKILL.md`, `plugins/pi/skills/clax/SKILL.md` (and the Grok plugin's copy if `main` has one), `README.md`, `scripts/test-plugins.sh`, `scripts/smoke-capabilities.sh`, `justfile`, `scripts/test-justfile.sh`
- Create: `scripts/demo-room-sample.sh`

**Interfaces:**
- Consumes: everything Tasks 1–8 built; `scripts/smoke-capabilities.sh`'s skeleton (`mktemp`, `KEEP`, the cleanup trap, the stale-`dist` refusal, `CLAX_*`, `smoke: ok <check>`).
- Produces: the `## Runtime capabilities` shared section naming `room` and `sample`; `docs/contract.md` "Room protocol" and "Sample protocol"; the `config.toml` section with `[sample]`; the `room` and `sample` checks in `smoke-capabilities.sh`; `just demo-room-sample`.

- [ ] **Step 1: Write the failing plugin check**

In `scripts/test-plugins.sh`, after the `same_section` check of `Runtime capabilities`, add a check on the first skill copy: the section names `room` and `sample` in its list of contract files, has a bullet starting `` - `room` `` and one starting `` - `sample` ``, and no longer contains `Rooms and \`sample()\` (phase 5)`; each failure prints `fail "<what is missing>"` as the script's other checks do.

- [ ] **Step 2: Run it to verify it fails**

Run: `scripts/test-plugins.sh`
Expected: FAIL on the new checks only.

- [ ] **Step 3: Edit the shared section, identically in every copy**

In `## Runtime capabilities` (`docs/contract.md` and each skill copy, byte-identical):

1. The contract fetch list "(names: `claude`, `permissions`, `artifact`, `db`, `downloads`, `user`, `comments`, `assets`)" gains `room` and `sample`.
2. "it resolves `null` for a name the page did not declare, for `files`, `mcp`, `room`, and `sample`, and outside the Clax viewer" becomes "…for `files` and `mcp`, and outside the Clax viewer".
3. After the `assets` bullet, add:

```markdown
- `room`: `emit`, `on`, `presence`, `peers`, `onPeers`, `join`, `connected`,
  and `onConnection` reach every view of the artifact that is open now;
  nothing is stored. Presence works for every viewer. A topic declared
  `{"room": {"topics": {"<topic>": "interact"}}}` may be sent on by a viewer
  on another machine who entered a name; every other topic only by the
  person's browser.
- `sample`: `sample(input, options)`, `sample.json`, and `sample.limits` ask
  Claude with the API key configured on the person's machine, and only in
  the person's own browser: everywhere else, and when no key is configured,
  `use("sample")` resolves `null`. The first call in each view asks the
  person to allow it.
```

4. In `### Differences from claude.ai`, add:

```markdown
- `room`: there are no agent peers (`kind` is always `"viewer"`);
  `sendToClaudeSession` rejects `claude_unavailable` and
  `canSendToClaudeSession` answers `"off"`. A new version does not empty
  the room: each view stays on its version until it reloads. A viewer's
  level is fixed per connection; entering a name reconnects at the new level.
- `sample`: the person's own API key pays for every call, not the viewer's
  account, so only their browser can call it. Allowing lasts for the view,
  not for the artifact. Answers are cached per browser in the daemon and
  forgotten, with the day's call counts, when it restarts. A daemon
  restart under an open tab ends its calls with `session_expired`.
```

5. Delete the bullet `- Rooms and \`sample()\` (phase 5): not available.` wherever it appears (the shared section's copies and `docs/contract.md` "What is not yet available"; keep the `files` and `mcp` bullet there).

- [ ] **Step 4: The rest of `docs/contract.md`**

In `## Runtime capabilities in detail`, "Everywhere: `files`, `mcp`, `room`, and `sample` resolve `null`" becomes "Everywhere: `files` and `mcp` resolve `null`". Add two sections after it, copied from this plan's Shared contract: `## Room protocol` (the socket, its frames, close codes, budget and bounds, the relay; and that the shell closes a document's socket when the document leaves) and `## Sample protocol` (the four routes with their callers, the refusals, the SSE frames, the relay, and the shell's mapping of refusals: 401 to `session_expired`, 404 to `not_declared`, 413 to `prompt_too_large`, any other code outside `SampleErrorCode` to `upstream_error`). In `### The daemon's port` (retitle it `### config.toml`), after the `[serve]` text, add the `[sample]` table of the Shared contract with: "A `[sample]` table that is invalid turns sample off and the daemon starts anyway; `clax doctor` prints it as a `warn` line. The key is read from the named environment variable once, when the daemon starts, and is sent only to `base_url`." In the `clax doctor` text, name the `sample` line.

- [ ] **Step 5: Amend the spec**

In `docs/superpowers/specs/2026-09-28-clax-design.md`, each change stated as the contract, not as history:

- §5 (`config.toml`): `[serve] port` and `[sample]` (the keys and defaults of the Shared contract); an invalid `[sample]` turns sample off and never stops the daemon.
- §9, wherever it says "`[server] bind` and `port`": `[serve] port`; the bind address comes from `--bind` only.
- §6: the room socket, the three sample routes with their callers (the token and a viewer cookie for the call and the tool result), and `GET /api/sample`.
- §8 "Time to usable": "capability members on the first `claude.use` the shell grants" names the parts: `caps` for most capabilities, `room` and `sample` each in a part of its own; the shell's `room` and `sample` handlers are chunks loaded on first use. `clax:degraded.part` lists `room` and `sample`.
- §9 `room`: the shell owns the socket in both frame modes; a `peers` snapshot lists at most 256 peers; after a subscriber falls behind it gets a fresh snapshot (scan N11); a version that stops declaring `room` closes sockets `revoked`; the socket closes when the frame's document leaves.
- §9 `sample` and §14: replace "consent is per viewer per artifact" with: only the owner's browser (the token and a viewer cookie) spends the key, and every call waits on consent given in that view; the shell shows a running count of calls. Record the sandbox ruling of this plan under §14's frame-gate text.
- §17 "Phase 5": what shipped. §18: `daily_call_cap` is off by default.

- [ ] **Step 6: Run the plugin check to verify it passes**

Run: `scripts/test-plugins.sh`
Expected: PASS.

- [ ] **Step 7: The scripted smoke**

Extend `scripts/smoke-capabilities.sh` (same scratch home, daemon and conventions; it writes `$CLAX_HOME/config.toml` with `[sample]\nprovider = "stub"\nstub_delay_ms = 20` before it starts the daemon). After its existing checks, add, each printing `smoke: ok <check>`:

- `sample-status`: `GET /api/artifacts/<aid>/sample` for an artifact declaring `{"sample": {}}`, with the token, answers `available: true, provider: "stub"`; without it, `available: false`.
- `sample-call`: `curl -sN` with the token and a cookie `clax_viewer=smoke` posts `{"input": "hello"}` and the stream ends with a `done` frame whose `text` is `echo: hello`.
- `sample-owner-only`: the same call without the token answers 401, and with the token but no cookie 403 `forbidden`.
- `sample-doctor`: `"$BIN" doctor` prints a `sample` line naming `stub`.
- `room-socket`: a short Node script (`node --input-type=module -e`, Node's built-in `WebSocket`) opens two sockets on an artifact declaring `{"room": {}}` with labels `aaaaaaaaaaaaaaaa` and `bbbbbbbbbbbbbbbb`, checks that the second's first `peers` frame lists both, closes the first, and checks the second hears `left`.

Its browser half (unless `--no-browser`) gains `room.spec.ts` and `sample.spec.ts`. Update the header comment to name the new checks.

Run: `cd web && npm run build && cd .. && scripts/smoke-capabilities.sh`
Expected: every check `ok`; the last line `smoke: all checks passed`.

- [ ] **Step 8: The demo**

`scripts/demo-room-sample.sh`: starts a daemon in a fresh `mktemp -d` home on `--port 0` (the `smoke-capabilities.sh` skeleton: the cleanup trap, `CLAX_CODEX_BIN=`, `CLAX_NO_OPEN=1`), writes `[sample]\nprovider = "stub"` unless `ANTHROPIC_API_KEY` is set in its environment (then it leaves `[sample]` at its defaults and says the real key will be spent), publishes `web/e2e/pages/room.html` and `web/e2e/pages/sample.html` with their capabilities, prints both URLs and "Ctrl-C stops the daemon and removes its home", and waits. It never uses `~/.clax`, `~/.clax-dev` or ports 7480, 7481 or 7490.

In the `justfile`, add with a description comment, as every recipe has:

```just
# Try rooms and sample() in a scratch daemon (the stub provider unless ANTHROPIC_API_KEY is set)
demo-room-sample:
    scripts/demo-room-sample.sh
```

and add `demo-room-sample` to the recipe list `scripts/test-justfile.sh` expects. In `README.md`, beside the `[serve]` text, add the `[sample]` table (provider, `api_key_env`, `daily_call_cap`), that only the person's own browser spends the key, that `clax doctor` reports it, and `just demo-room-sample`.

Run: `scripts/test-justfile.sh && bash -n scripts/demo-room-sample.sh`
Expected: PASS. The implementer does not run the demo with a real key.

- [ ] **Step 9: Run everything**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cd web && npm test && npm run typecheck && npm run lint && npm run build && node scripts/bundle-size.mjs && npx playwright test && npm run perf && cd .. && scripts/test-plugins.sh && scripts/test-justfile.sh`
Expected: PASS.

- [ ] **Step 10: Stage**

Stage the change (`git add docs plugins README.md scripts justfile`); do not commit. The controller commits it with the message:

```text
Document rooms and sample() for pages and agents, amend the spec, and add their smoke checks and a scratch-daemon demo
```

---

## Ship criteria

- Every task's tests pass, and Task 9 Step 9's full run passes on `main` after the controller's commits.
- In both frame modes: two tabs share presence, messages and named rooms; a document that leaves takes its peer with it at once; a LAN viewer who names themselves can send on an `interact` topic.
- The owner's browser gets the consent dialog once per view, streamed text, page tools, Stop and the count; a LAN viewer's `use("sample")` resolves `null`; no request without the token and a cookie reaches the provider.
- No budget in `web/perf/bundle-budget.json` or `web/perf/budget.json` was raised; `partRoom` and `partSample` exist; a page that uses neither capability loads neither part nor either shell chunk.
- `clax doctor` reports the `sample` line, and an invalid `[sample]` warns without stopping the daemon.
- The open questions in `open-questions.md` were answered by the owner, or their recommended answers stand as built.
