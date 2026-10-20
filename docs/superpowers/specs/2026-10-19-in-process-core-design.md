# The Clax app's core in its own process: no daemon, no HTTP

Date: 2026-10-19. Branch: `flutter-app`. Status: draft for the owner's review,
revised the same day with the owner's decisions 8 to 13.
Builds on `2026-10-12-flutter-app-design.md` (the "app spec"),
`2026-10-12-flutter-app-round2-design.md` (the "round-2 spec"),
`2026-10-18-ios-release-design.md` (the "iOS spec") and `docs/bazel.md`
("The Clax app"). It supersedes the parts of them named in §13.

## Goal

The Clax app (`apps/clax_app`) runs Clax's core, the Rust store and router
that `clax serve` runs, as a library inside its own process on Android,
macOS, iPhone and iPad. Nothing listens on a port, nothing is spawned,
nothing is probed for health. The app's agent and its queue of makes and
changes run in a long-lived Dart worker isolate, as one model in the Elm
architecture's shape, over durable job steps the core keeps in SQLite. The
person never sees transport state for local work.

After this work:

- one crate, `crates/clax-embed`, is linked into the app on all three
  platforms and exposes four C functions (§2.2);
- the app's WebViews load the shell, pages and files from a scheme the app
  answers (§4), and the shell talks to the core over the `ClaxHost`
  channel in app mode (§5); the browser shell is unchanged;
- each artifact's content is a separate origin from the shell and from
  every other artifact (§4.3);
- makes and changes are jobs in the core's database, which are the only
  truth about them; a step that publishes commits with its version, so a
  job resumed after a kill never publishes twice, and one queue owner per
  core runs them (§7, §8.1);
- the icon and page checks that `finish` applies are the core's, so every
  publish obeys them (§8);
- the seven JSON state files are imported once into core tables (§9);
- the daemon supervisor, the port, the key cookie, the health probe and
  `crates/clax-ios` are deleted (§13).

## Owner's decisions (binding)

1. **No separate daemon process and no HTTP inside the app.** Android and
   macOS stop running `clax` as a child; the supervisor, health probes,
   port picker and port file, key cookie, `ExeLocator` and sample-key HTTP
   go. iOS's `crates/clax-ios` (a thread serving loopback HTTP) is replaced
   too. No loopback listener anywhere in the app.
2. **One Rust crate, `clax-embed`**, linked on Android, macOS and iOS, with a
   fixed C ABI of about five functions (`open(home)`, `call(request bytes)
   → response bytes`, `free`, `close`, `subscribe(dart port)`). `unsafe`
   only on those exports and their buffer handling; `scripts/check-no-unsafe.sh`,
   its test and AGENTS.md name exactly those exports; the clax-ios
   exception goes. `call` feeds a serialized request into the existing
   axum Router in-process; the core's background tasks run on its own
   threads; events go to Dart ports. Layering per `scripts/check-crate-layers.py`.
3. **WebView without a server.** A custom scheme answered by the host
   (Android `shouldInterceptRequest`, `WKURLSchemeHandler` on Apple). API
   calls in app mode go over `ClaxHost`, answered by `runJavaScript`. Events
   are host-pushed through the `claxHost` path. The app never shows "Live
   updates paused. Reconnecting…" or "Clax is not answering. Retrying…".
   The browser shell is unchanged. Each artifact's content is its own
   origin, and the security model is spelled out.
4. **The queue lives in a long-lived Dart worker isolate**, one Elm-shaped
   model: Jobs with state and step, a pure `update(model, msg) → (model,
   effects)`, an effect runner. The UI isolate never awaits work: it
   receives `QueueState` and sends intents (make, change, cancel, retry,
   keep going, discard). The agent stays Dart and moves into the worker.
   Rendering is asked of the UI isolate by message. Clax's own agent is
   app-only for now.
5. **Shared logic goes in Rust**: durable job steps (publish atomic with the
   version), the icon and page checks, and the JSON state files as core
   tables (the core owns what it should; app-only preferences may stay).
6. **Background:** jobs keep running when the person leaves the app: the
   Android foreground service (`ChannelBackgroundWork`) holds the process
   while any job runs; Mac and iOS go on while the OS allows. After a kill,
   each job resumes from its last committed step at the next launch (the
   WorkManager wake first named here was dropped by decision 12).
7. **No transport state for local work.** "Waiting for connection" remains
   only for the Claude API, worded to name Claude; the home tile's
   "Offline" says what is unreachable.

Taken on 2026-10-19, after the first draft:

8. **Domain effects, not transport effects.** `update` emits domain
   operations (`LoadJobs`, `StartTurn`, `RunTool`, `Render`, `CommitStep`,
   `Publish`, `Reply`, `Notice`, `KeepAlive`, …), never a core call. Only
   the effect runner maps an effect to core calls. `update`'s tests assert
   meaning, never paths or methods (§7.1).
9. **One source of truth.** The core's job rows are the truth. The worker's
   model is a projection rebuilt from `JobChanged` events and `LoadJobs`,
   plus the effects in flight. `update` never advances a job's durable
   state except by emitting a commit effect and applying the core's answer,
   or the event that answer causes (§7.4).
10. **No wall-clock leases.** One queue owner per core (per job client,
    `app` for the app's queue): the core keeps an in-memory owner claim
    that a worker isolate takes at its start, refused to a second claimant
    while the first's isolate lives, and released when that isolate's port
    closes or the core closes or reopens. The jobs API
    stays generic: an owner is any single caller of the core, so the
    localhost daemon can serve it later (§7.5, §8.1).
11. **The queue is built once, on its real store.** The core's jobs and
    steps land before the queue, and the queue is written directly on them
    (§17).
12. **No Android WorkManager wake and no headless engine.** Jobs resume at
    the next launch on every platform; the foreground service keeps running
    jobs alive on Android, as before (§7.6).
13. **This branch's migrations follow main's numbering**, and installed
    homes at this branch's earlier numbering are adopted by their schema
    after a backup, never re-run (§10.2).

## Decisions this spec takes

Where the owner's text leaves a choice, this spec takes the reading below.
Each marked **(ask)** is also a `status:needs-decision` record on this file.

- **S1. `call` answers through a Dart port, not a return value (ask).**
  Several routes the app uses wait by design: `GET /api/sessions/{sid}/feedback?wait=`
  (`pollFeedback`), `waitQuestion`, and a 16 MiB publish or a migration can
  take seconds. A `call` that returned its response would block the calling
  isolate's thread for that long; on the UI isolate that is dropped frames,
  and on the worker it stalls the Messages API stream. So `call` takes the
  request bytes and a Dart port, returns at once, and the core posts the
  response bytes to that port (a copy Dart owns; §2.2). With nothing handed
  back to free, `free` has no use, and the ABI is four functions: `open`,
  `call`, `subscribe`, `close`. The alternative (a blocking `call` and `free`,
  run from a pool of helper isolates) is recorded as an `alternative`.
- **S2. Platform URLs.** One logical scheme, `clax:`, written per platform
  (§4.1): Android answers `https://*.clax.invalid` in `shouldInterceptRequest`
  (a custom scheme gets an opaque origin in Android's WebView, which breaks
  `localStorage`, `postMessage` targets and CORS); Apple answers `clax://`
  with `WKURLSchemeHandler`.
- **S3. The channel is the shell's alone.** The host accepts `ClaxHost`
  messages only from the shell's origin in the main frame (§4.4). Android
  uses `WebViewCompat.addWebMessageListener` with that origin as its one
  allowed rule, replacing `addJavascriptInterface`; Apple checks
  `WKScriptMessage.frameInfo`.
- **S4. The core owns the queue's durable state; the app keeps preferences.**
  Core tables: jobs, their steps, the threads and questions they answer, and
  the agent's remembered read consents (§9). App-side and unchanged:
  `settings.json`, `timings.jsonl`, `agent-timings.jsonl`, `lineages.json`,
  `home.json`, `device-grants.json`, `device-reminders.json`, `bundles/`,
  `snapshots/`. There are seven state files, not six: `unfinished.json`
  (`lib/agent/unfinished.dart`, Keep going's carried draft) is a job's state
  too and moves with them.
- **S5. A render-clean record is the core's draft gate.** The core cannot
  render, so the `finish` gate's "the draft's latest revision rendered with
  no uncaught errors and no failing tests" becomes: the app records each
  render's result against the draft's content digest, and a publish step
  made from a draft is refused unless that digest has a clean render (§8.2).
- **S6. Jobs resume at the next launch, on every platform (decided).** The
  owner chose "resume at launch; no wake" on 2026-10-19 (decision 12): no
  WorkManager wake and no headless engine on Android, so every render has
  the app's UI to run in. A job the OS or a kill interrupted waits, as its
  core row left it, for the next launch.
- **S7. iOS gains no BGProcessingTask (ask).** There is none today (the iOS
  spec took `beginBackgroundTask` and turned down `BGContinuedProcessingTask`).
  iOS and the Mac keep `IosBackgroundWork`'s pause at the time limit, and
  resume at the next launch.
- **S8. The Android engine outlives the activity.** The foreground service
  holds the process; a `FlutterEngine` kept in `FlutterEngineCache` by the
  application holds the isolates, so Back out of Clax does not end a job
  (today the activity's own engine dies with it).

## 1. What runs today

| | Android | macOS | iOS |
|---|---|---|---|
| The core | `libclax.so`, the `clax` executable, as a child (`//crates/clax-cli:clax_android_*`, `bazel/android/clax.bzl`) | `clax` in `Contents/Resources`, a sandboxed helper (`macos_helper`, `bazel/flutter/macos.bzl`) | `crates/clax-ios`, a static library serving loopback HTTP on a thread |
| Driven by | `DaemonSupervisor` (`lib/daemon/supervisor.dart`, 727 lines): port file, `daemon.json`, `/healthz`, backoff, restart on a key change | the same | the same supervisor over `InProcessStarter` (`lib/daemon/in_process.dart`) |
| The app's calls | `HttpClaxApi` (`lib/clax/http_api.dart`) over `package:http` to `http://127.0.0.1:<port>` | the same | the same |
| Events | `SseClaxStream` / `SharedClaxStream` on `/api/stream` (`lib/clax/stream.dart`, `shared_stream.dart`) | the same | the same |
| WebViews | `http://127.0.0.1:<port>/a/<id>?app=1` with the `clax_key_<port>` cookie (`lib/ui/daemon_cookie.dart`); content at `http://<id>.localhost:<port>` | the same | the same, ATS allowing loopback |
| Queue state | seven JSON files in the support directory, written by the UI isolate | the same | the same |

`clax_server::serve::serve` (`crates/clax-server/src/serve.rs:344`) does, in
order: open the store (with its backup and migrations), write `daemon.json`,
assemble `AppState`, start the audit journal and the listeners
(`start_listeners`), build the router, spawn six tasks (stale watcher,
reaper, grant sweeper, optimizer, drafts sweeper, working sweeper), and
serve with graceful shutdown. Everything but the listener, `daemon.json`
and the stale watcher is what an embedded core needs.

## 2. `crates/clax-embed`

### 2.1 The crate

A new crate, `crates/clax-embed`, replacing `crates/clax-ios`. It is built
as a `cdylib` for Android (`libclax_embed.so` per ABI) and macOS
(`libclax_embed.dylib` in the bundle's `Frameworks`), and as a
`staticlib` for iOS (linked into the runner; iOS loads no loose dylib). Its layer is the
one clax-ios had: `"clax-embed": SERVER | {"clax-embedded"}` in
`scripts/check-crate-layers.py`'s `ALLOWED`, and `clax-ios` leaves the map.

```
crates/clax-embed/
  Cargo.toml          [lints.rust] unsafe_code = "deny" (the only crate but clax-worker)
  BUILD.bazel
  include/clax_embed.h   the four declarations (§2.2)
  src/lib.rs          the safe API: Embed, open/call/subscribe/close
  src/core.rs         Core: the store, AppState, tasks and router, without a listener
  src/frame.rs        request and response frames (§2.3)
  src/events.rs       a subscription: a stream hub connection posted as SSE blocks
  src/dart.rs         Dart_CObject and the post function's type (no unsafe)
  src/ffi.rs          the four exports; the only file with unsafe
  src/log.rs          the rolling log (moved from clax-ios), at <home>/logs/core.log
  src/web.rs          the embedded web UI as a FileTree (moved from clax-ios)
```

A core opened through `clax_embed_open` writes its rolling log to
`<home>/logs/core.log` (cut at 1 MiB to `core.log.1`), beside the home's
other logs; the C ABI names no log path.

`clax-server` gains `clax_server::core::{Core, CoreConfig}`: `serve()` is
split so that everything it does but binding, `daemon.json` and the stale
watcher is `Core::start(cfg) -> Core`, which owns the background tasks and
`Core::router()`, and `Core::stop()` ends them as `serve` does after its
drain. `serve()` becomes `Core::start` plus the listener and `daemon.json`;
`clax serve`'s behaviour does not change (its integration tests are the
check). `Core::start` runs on a Tokio runtime that `clax-embed` owns: a
multi-thread runtime with two workers, named `clax-rt`, as clax-ios had.

### 2.2 The C ABI

```c
// include/clax_embed.h
#include <stdint.h>
#include <stddef.h>
typedef struct Dart_CObject Dart_CObject;
typedef int8_t (*ClaxPost)(int64_t port, Dart_CObject *message);

// Pointers are never NULL: a NULL `home` or `post` makes clax_embed_open
// answer 0, and a NULL `req` makes clax_embed_call answer 2, before either
// reads anything. A length may be 0 with a pointer that is not NULL.

// Opens the core on the home at home[0..home_len] (UTF-8) and returns its
// handle at once (never 0). The store opens, backs up and migrates on the
// core's own threads; `port` hears {"type":"ready"} or
// {"type":"failed","code":…,"message":…}, then {"type":"closed"} after
// clax_embed_close. A home this process has open already (the app's own
// core) is not opened again. When the isolate that opened it is gone (it
// restarted without closing the core) the open re-attaches, returning
// that core's handle; its open events go to `port` from then on, which
// hears the latest at once ("ready", or where opening has got to). To
// tell, the open posts {"type":"probe"}, which is no event and which a
// client ignores, to the core's open port: the post fails only once that
// port's isolate is gone. A core whose open failed is never re-attached
// to. Opens are taken one at a time, so two at once of one home get one
// handle and one refusal. 0 when the path is not UTF-8 (`port` hears
// "failed" with code open_failed), that home's core has a live client
// (code home_in_use, the home open in this app) or is closing (code
// home_in_use), or `home` or `post` is NULL (nothing is posted). `post`
// is Dart's NativeApi.postCObject.
uint64_t clax_embed_open(const uint8_t *home, size_t home_len,
                         ClaxPost post, int64_t port);

// Sends one request frame (§2.3). Returns 0 when the core took it, 1 for
// an unknown handle, 2 for a frame that does not parse or a NULL `req`.
// The response frame is posted to `port` as one Uint8List: `tag` (8
// bytes, little endian) then the frame. A call made before "ready" waits
// for it. A cancel frame, {"cancel": <tag>}, is answered 204 at once
// under its own `tag`, after it aborts the call sent to this `port` with
// that tag, if the core is still running it: that call is never
// answered, and a write it had not committed does not happen.
int32_t clax_embed_call(uint64_t core, const uint8_t *req, size_t req_len,
                        int64_t port, int64_t tag);

// Posts the core's events to `port`, one Uint8List per SSE block of the
// stream hub (§2.4), until the port is closed or the subscription ends;
// an ending subscription posts `event: end` with {"reason":…} last
// (core_failed, core_closed, stream_refused with the refusal's "status",
// stream_ended, internal). 0, or 1 for an unknown handle.
int32_t clax_embed_subscribe(uint64_t core, int64_t port);

// Stops the core as `clax serve` stops on SIGTERM: new calls answer 503
// core_closed, streams and long polls are told to end, calls in flight
// finish for up to 5 s (then answer 503 core_closed), tasks stop, the
// journal syncs, the store closes. Returns at once; the open port hears
// {"type":"closed"}, after the answer of every call that finished in the
// drain has been posted. 0, or 1 for an unknown handle.
int32_t clax_embed_close(uint64_t core);
```

- A handle is a number in a process-wide registry (`Mutex<HashMap<u64,
  Arc<Embed>>>`), never a pointer, so nothing dereferences it. Any isolate
  may use it: the UI isolate opens the core and gives the handle to the
  worker isolate when it spawns it.
- `post` is called from the core's threads. `NativeApi.postCObject` is
  thread-safe and copies a typed-data message, so Rust frees its buffer as
  soon as `post` returns. A `post` that returns 0 (the port is closed) ends
  that subscription; a dropped call response is logged and forgotten.
- `Dart_CObject` is declared in `src/dart.rs` as `#[repr(C)]` with its
  union sized by its largest member (`as_external_typed_data`: type, length,
  data, peer, callback), matching `dart_native_api.h` of the Dart SDK that
  Flutter 3.47.7 ships; the core builds only `kTypedData` (type 7) with
  `Dart_TypedData_kUint8` (2). Building a union and passing a pointer to it
  is safe Rust; nothing reads one.

### 2.3 Frames

A request frame is `CLX1`, a 4-byte little-endian length, that many bytes
of JSON, then the body bytes:

```json
{"method":"POST","path":"/api/artifacts/abc/threads?x=1",
 "headers":[["content-type","application/json"]],
 "as":"owner" | "content" | "shell-page", "host":"<id>" | null}
```

`as` says who the request speaks for; the core, not the caller, turns it
into credentials:

| `as` | Who sends it | What the core adds | Paths it may reach |
|---|---|---|---|
| `owner` | Dart (the app's own code and agent), and shell calls the host relays (§5) | `Authorization: Bearer <token>` unless the frame carries an `Authorization` (an agent session's own); `Host: localhost`; `ConnectInfo` of a loopback peer | everything but `/api/admin/shutdown` and the routes that hold their connection open (`clax_server::router::held_open`: `/api/stream`, `/api/events`, `/api/artifacts/{aid}/room`, `/api/artifacts/{aid}/live`, `/mcp`), since a call's body is read whole; events go by `subscribe`, and `POST /api/stream/{id}` is allowed |
| `shell-page` | the host, for a scheme request to the shell's origin (§4.2) | `Host: localhost`; for the shell document (`/a/…`) alone, the owner cookie (`clax_owner_80`), so the shell document is the owner's browser's (§4.3), and no credential for the rest; of the frame's headers, only `Range`, `If-None-Match`, `If-Modified-Since`, `Accept` and `Service-Worker` | the shell document and what the core serves it only: `/a/…`, `/_clax/…` (the web UI's files), `/c/…`, `/_blob/…`; never `/api/` |
| `content` | the host, for a scheme request to an artifact's origin | `Host: <host>.localhost`; no credential; of the frame's headers, only `Range`, `If-None-Match`, `If-Modified-Since`, `Accept` and `Service-Worker` | what `rewrite_artifact_host` admits on an artifact host (`/v/…`, `/draft/…`, `/_blob/…`, `/_clax/…`) |

A **cancel frame** is a frame whose JSON is `{"cancel": <tag>}`, with no
body, sent through `clax_embed_call` on the port of the call it names:
the core keeps each call's task by port and tag until the call has its
answer, and a cancel aborts that task (dropping the router's future at
its next await, so a long poll that has not taken its batch takes none,
and the call is never answered), then answers the cancel 204 under the
cancel's own tag. A cancel of a call already answered, or of none, is
answered 204 and does nothing more. A call the caller gives up on is
cancelled this way, so cancelling needs no fifth C function: Dart's
`CoreCalls.call` answers a `CoreCall` (its tag and its answer) and
`CoreCalls.cancel(tag)` sends the frame, and `CoreTransport` cancels a
call whose time limit passes, as a loopback timeout aborts its request.

A response frame is `CLX1`, a length, JSON `{"status":200,"headers":[…]}`,
then the body. Bodies are read whole up to 96 MiB (`PUBLISH_BODY_LIMIT`);
a larger body answers 502 `response_too_large`, at once and unread when
its size hint already says it is larger. The core copies a request
once (out of the buffer Dart lends for the call) and a response once
(each body chunk into the one buffer it posts, after the tag and the
head, sized from the body's size hint; a body whose size is not known
ahead may make that buffer grow, and be copied again as it does);
`postCObject` copies that buffer again into the Dart heap. A request to a refused path
answers 404 `not_in_app` without reaching the router.

Dart never holds the token: the core adds it. The core sets `require_credential` as the
app's daemon did. Any frame of the WebView can make a request to the
shell's host (an artifact page's `<script src>`, `<img src>` or fetch of
a `clax://app/…` URL included), and each reaches the core as a
`shell-page` frame. So a `shell-page` frame speaks as the owner's browser
(the owner cookie) only for the shell document, `/a/…`, which the host
answers to the main frame's navigation alone (§4.2), as the daemon app's
WebView carried the owner's key cookie on it: the shell document's boot
data is the owner's as it was there. The rest a `shell-page` frame
reaches (`/_clax/…`, `/c/…`, `/_blob/…`) answers on the daemon without a
credential (`auth::open_without_credential`) and is asked for with none,
so a page that includes it gets nothing the owner's credential would add.
A `content` frame never carries a credential: it reaches the router as a
viewer without one, which is what a page's frame is on the daemon today.

### 2.4 Events

`clax_embed_subscribe` sends `GET /api/stream` through the router as an
`as: owner` request (with a loopback `ConnectInfo`, so the route's
`sees_live_pages` judges it as it judges the app's daemon today), with the
subscriber's resume point as `Last-Event-ID` when it has one (a shell
document's `subscribe` carries it as `lastEventId`, §5.2): the hub then
replays what that stream missed, or answers `resync`, as it does for an
SSE client that reconnects. It pumps
the streaming response body: each chunk the hub's `Conn::next` yields
(`crates/clax-realtime/src/stream.rs:1783`; one SSE block, `id:`, `event:`,
`data:`, keep-alives included) is posted as one message. Readers buffer to
the blank line anyway, so a block split across messages still parses. The first block is the
hub's `ready` with the stream's ID; the last, unless the port closed
first, is `event: end` with `{"reason":…}`: `core_failed` (the core did
not open; at once for a handle whose open failed), `core_closed`,
`stream_refused` (the router of a ready core refused the stream, with
its status: `{"reason":"stream_refused","status":503}`), `stream_ended`
or `internal` (the stream panicked while it was read; a route that
panics answers 500 `internal` instead, so its stream ends
`stream_refused` with 500), so each consumer, including one on an isolate with no
open port, knows to resubscribe or stop. As a loopback SSE client does,
a consumer stops for good only on `stream_refused` with 400, 403 or 404,
which asking again cannot change; after `stream_ended`, `internal` or any
other refusal it subscribes again with the stream's backoff (0.5 s
doubling to 30 s, reset by the next `ready`). On `core_closed` or
`core_failed` a Dart consumer stops (reopening the core is its host's
work, and the host subscribes it again then); a shell document, which
cannot see the core reopen, subscribes again with the backoff, so it
heals once the host has reopened the core (§5.2). Topic changes go over `call` as
today's `POST /api/stream/<id>` (`as: owner`). A subscriber that falls
behind gets the hub's `resync`, as an SSE client does. The SSE route and
the subscription share the hub's connection type; the hub gains only what
the queue's owner claim needs (`Hub::closed` and `Hub::nudge`, §8.1).

The app subscribes once per consumer: the UI isolate's `ClaxStream`
(replacing `SharedClaxStream`), the worker isolate's queue (whose stream
also holds the queue's owner claim, §7.5), and each WebView's shell
document (§5.2).

### 2.5 Opening a home

A home this process holds already is not opened again: the only holder
in one process is the app itself, so `clax_embed_open` re-attaches to that
core (§2.2) when the isolate that opened it is gone (a hot restart, or an
isolate that ended without closing it). It posts `{"type":"probe"}` to the
core's open port first, a message that is no open event and that
`CoreClient` drops, and re-attaches only when that post fails, as it does
once the port's isolate is gone. A core whose client lives (a second
isolate that calls `open` rather than `attach`) answers 0 with `failed`
code `home_in_use` saying the home is open in this app, and so does a
core that is closing; a core whose open failed released the home and is
skipped, so the home opens afresh. Opens are taken one at a time, an
open's `Embed::open` and its handle's registration together, so two opens
of one home at once get one handle and one `home_in_use`. Otherwise `Embed::open(home)`, on the core's runtime:

1. takes `<home>/core.lock` with `std::fs::File::try_lock` (stable since
   Rust 1.89; no unsafe). Held: `failed` with code `home_in_use` (another process
   holds it: a second Mac app instance, or a test that forgot to close);
2. retires a daemon of the old app: if `<home>/daemon.json` exists it is
   renamed to `daemon.json.retired`; a `clax serve` child of an earlier app
   build still running sees its file missing on two stale checks and exits
   (`serve.rs`'s stale watcher), and its port, token and cookie die with it.
   `<support>/daemon-port` is deleted by the app (§10);
3. writes `config.toml`'s `[sample]` section as the supervisor's
   `sampleConfig` did (moved to `CoreHost`, §6);
4. opens the store with `Store::open_reporting`: the pre-migration backup
   (`clax-store/src/backup.rs`), `adopt_earlier_flutter_app`, the migrations,
   progress reported on the open port as `{"type":"opening","phase":…}`:
   `backing up` before the copy, `migrating` with `"done"` and `"total"`
   before each migration, then the audit backfill's phases (`clax serve`'s
   `starting.json` counts the copy and the migrations as `opening`);
5. assembles `AppState` with `embed: Some(EmbedOrigins)` from
   `<home>/embed.json` (§4.1, §4.3) and starts
   the core (§2.1); posts `ready`.

A failed migration posts `failed` with `code: "migration_failed"` and the
backup's path; the app shows its existing migration-failure screen
(today's exit 65 path), never a connection message. A panic in the
opening task posts `failed` with code `internal` and releases the home; a
panic in the close task still cuts the calls left, stops what it can,
releases the home and posts `closed`.

### 2.6 Trimming memory

`clax_trim_memory` goes with clax-ios. Its work (`Store::trim_memory`:
`PRAGMA shrink_memory` on every connection and a passive checkpoint) is a
route, `POST /api/core/trim` (`as: owner`), which the Dart side calls when
`ios/Runner/Background.swift` reports that the app left the screen or got a
memory warning (it posts `trim` on `dev.empathic.clax/background`, which
Dart answers by the call), and on Android's `onTrimMemory` the same way.

## 3. The key

The Claude API key is put in the core's `KeySlot` over `call`
(`PUT /api/sample/key`, which clax-ios added) at open and on every change,
on every platform. A key change no longer restarts anything (record
69557070 is fixed for Android and the Mac; record c58da88f's unretried put
becomes a `call` that cannot fail on transport, and its 4xx is logged).

## 4. The WebView scheme

### 4.1 URLs

| Logical | Android (`shouldInterceptRequest`) | Apple (`WKURLSchemeHandler`) |
|---|---|---|
| the shell | `https://app.clax.invalid/a/<id>?app=1` | `clax://app/a/<id>?app=1` |
| an artifact's content | `https://<id>.c.clax.invalid/v/<n>/…` | `clax://<id>.c/v/<n>/…` |

`.invalid` is reserved (RFC 2606): if interception ever misses a request,
it fails to resolve rather than reach a network. One Dart function,
`lib/core/origins.dart` `AppOrigins.forPlatform()`, gives the shell origin
and the content origin of an ID. The app writes both to `<home>/embed.json`
(`{"shell": …, "content": …}`, the content one a template with `{id}`)
before `clax_embed_open`, which reads it (`failed` with `no_origins`
without it); the core keeps them as `EmbedOrigins` and uses them in the
shell documents and in the content policy.

### 4.2 Answering a request

The host answers every request to the shell's host or a `*.c` host; other
hosts are not intercepted (the content policy already refuses them).

1. Map it to a frame: method `GET` or `HEAD` only (a scheme request with
   another method answers 405: a page's writes go through the shell's
   bridge, never its own origin), `as: shell-page` for the shell's host,
   `as: content` with `host: <id>` for `<id>.c`, the request's
   `Range`, `If-None-Match` and `Accept` headers, no others.
2. `call` it from the UI isolate (the WebView's platform thread hands the
   request to Dart and waits on the answer; Android's `shouldInterceptRequest`
   runs on a WebView IO thread and blocks on a `CompletableFuture` that the
   Dart reply completes; Apple's handler completes its task when Dart
   answers).
3. Answer with the frame's status, headers and body, minus every
   `Access-Control-*` header (§4.3), and with `Cache-Control` as the core
   sent it.

A shell document (the HTML answer to `/a/<id>`, the only document the app
loads on the shell's host: it loads no gallery documents, so `/`, `/inbox`
and `/people` are not shell documents in the app, and a `shell-page` frame
cannot reach them) is answered only to a main-frame navigation; any other
request for one answers 403. On Android the handler reads it from the
request (`WebResourceRequest.isForMainFrame()`). On Apple a
`WKURLSchemeTask` names no frame, so the navigation policy decides it
before the handler is asked: `webView(_:decidePolicyFor:)` cancels every
navigation to the shell's host whose `navigationAction.targetFrame` is not
the main frame (a `nil` target, a new window, included), and the handler
answers a shell document only to a request whose `mainDocumentURL` is its
own URL. A page cannot frame or fetch the shell document.

### 4.3 Origins and the security model

What holds on the daemon today (`docs/contract.md`, "Security model") holds
in the app, with the scheme in place of the port:

- **Each artifact's content is its own origin** (`https://<id>.c.clax.invalid`
  or `clax://<id>.c`), distinct from the shell's and from every other
  artifact's. Its storage, cookies and service workers are its own (the
  kill switch for service workers stays: `Service-Worker: script` is
  answered with `SERVICE_WORKER_KILL_SWITCH`).
- **The content policy is the daemon's**, with `frame-ancestors` naming the
  shell's origin instead of `http://localhost:<port>` (`content_policy`
  takes the embed's shell origin when `AppState.embed` is set). The load
  lockdown (`'self'` only) is unchanged.
- **The shell document is the owner's; a page's are no one's.** A
  `shell-page` frame carries the owner cookie only for the shell document
  (`/a/…`), which the host answers to the main frame's navigation alone
  (§4.2). Any frame, a page's included, can make the host send a
  `shell-page` frame for the shell's other paths (`/_clax/…`, `/c/…`,
  `/_blob/…`, by `<script src>`, `<img src>` or fetch), so those carry no
  credential and answer as they do on the daemon without one. A
  `shell-page` frame reaches only read-only paths (`GET` and `HEAD`), and
  a page cannot read what a cross-origin request answers. A `content`
  frame never carries a credential and is never the owner.
- **A page reaches nothing with the owner's authority.** Its requests are
  `as: content` (no credential, artifact paths only). It cannot read the
  shell's responses: the host strips `Access-Control-*` from every
  intercepted response, so the router's permissive CORS layer never reaches
  a WebView, and cross-origin reads are opaque. It cannot load the shell
  document (§4.2). It cannot reach `/api/` by any URL: the shell host
  refuses it (`shell-page` never reaches `/api/`).
- **Only the shell's main frame reaches the channel** (§4.4), so only the
  shell's own code makes owner calls, as on the daemon where only the
  shell's origin holds the key cookie. A page's capability calls go, as
  today, through the shell's bridge (`bridge-link.ts` checks `e.source` and
  `e.origin`), which applies the capability checks and marks them
  `x-clax-via: page`.
- **Other apps on the device reach nothing.** There is no port, so the
  daemon's token and cookie defences against other local apps have nothing
  to defend.

What the spike checks on each platform (Task 14 of the plan; on Android an
integration test run on an emulator under Bazel, on the Mac one run by
`just app-integration`, each asserting every value) before the
scheme replaces the port: `window.origin` in the shell and in a content
frame; `localStorage` and IndexedDB surviving a reload and an app restart
per origin; a content frame's `parent.document` access throwing; a
`postMessage` from the frame arriving with the content origin as
`event.origin`; `crypto.subtle` present (a secure context); a content
frame's `fetch` of the shell's `/a/<id>` and of `/api/artifacts` failing
or reading nothing. On Apple, if WebKit gives a registered custom scheme
an opaque origin, the spike stops and the owner is asked: WebKit refuses a
`WKURLSchemeHandler` for `http` and `https`, so the only other way to give
pages real origins on Apple would be a loopback listener, which decision 1
forbids.

### 4.4 The channel

- **Android:** `ClaxHost` becomes a `WebViewCompat.addWebMessageListener`
  (androidx.webkit, `WebViewFeature.WEB_MESSAGE_LISTENER`) with the one
  allowed origin rule `https://app.clax.invalid`, which the WebView injects
  only into frames of that origin; the listener also drops a message whose
  `isMainFrame` is false. `window.ClaxHost.postMessage(string)` keeps its
  shape in JavaScript. webview_flutter's `addJavaScriptChannel` (an
  `addJavascriptInterface`, injected into every frame) is no longer used
  for it.
- **Apple:** the `ClaxHost` script message handler drops a message unless
  `message.frameInfo.isMainFrame` and its `securityOrigin` is the shell's
  (`clax`, `app`).
- The nonce handshake (`ShellSession`, `host_protocol.dart:1157`) stays as
  a second check. The nonce comes with the document and is keyed to the
  main frame's navigation, not to its URL. The host numbers each
  main-frame navigation to the shell's host, and gives that number with
  the navigation's own request and with its commit. The scheme handler
  (§4.2) makes a fresh nonce (16 to 128 of `[A-Za-z0-9_-]`) only for a
  shell document requested under a navigation's number, writes
  `<meta name="clax-nonce" content="…">` right after its `<head>`, and
  keeps it with that number. The surface takes a nonce as the load's only
  when the navigation of that number commits, and gives the same one to
  `claxHost.init`. A nonce whose navigation never commits (cancelled,
  superseded or failed) is dropped, and a request with no navigation's
  number gets no shell document (403). The shell's transport reads the
  nonce at boot, so its calls go out before the document's load event
  (which waits for the artifact page's own load); only a document without
  one waits for `claxTransport.init` (§5.1). How a request is paired with
  its navigation:
  - **Android:** the number goes with a request only when
    `WebResourceRequest.isForMainFrame()`, which no frame inside the page
    can make; the host keeps the latest such request's number and URL (a
    newer main-frame request replaces it, as the WebView supersedes the
    older navigation), and `WebViewClient.onPageStarted`, which the
    WebView calls for the main frame when its navigation commits, reports
    that number when its URL is the kept one's.
  - **Apple:** a `WKURLSchemeTask` names neither its frame nor its
    navigation. `webView(_:decidePolicyFor:)`, when it allows a main-frame
    navigation to the shell's host, opens a one-shot permit with a new
    number for that URL (closing any earlier one);
    `webView(_:didStartProvisionalNavigation:)` binds the open permit's
    number to that `WKNavigation`. The scheme handler gives the number
    only to the first request for the permitted URL whose
    `mainDocumentURL` is that URL, and closes the permit: every later
    request for it goes without one (403) until the next permit.
    `webView(_:didCommit:)` reports the number bound to the committing
    `WKNavigation`, compared by identity, and
    `didFailProvisionalNavigation` drops it.
  - **What remains.** On Apple a request is paired with its navigation by
    order, since WebKit links them by nothing. While a reload or follow
    of the shell is in flight the outgoing document and its content frame
    still run, and a request one of them made for exactly the shell
    document's URL, between the policy's decision and the navigation's
    own request, would take the permit: the navigation's own request
    then answers 403 and that load fails, with no shell in what commits.
    The page cannot read the nonce it was answered (the response is
    cross-origin and carries no `Access-Control-*`, §4.3), and what
    commits under that number is the 403, with no shell to use it; so
    the residual is the loss of that load, and what keeps a page from making
    the request at all is the content policy (`default-src` and
    `connect-src` name only the page's own files and `/_blob/<id>/`). On
    Android `onPageStarted` names only a URL: should an earlier
    main-frame navigation to the same URL commit after a later one's
    request, it is reported under the later one's number, and its
    document's meta does not match, so its messages are dropped until the
    later one commits.

How the host gets at each WebView's configuration is the spike's other
question: on Android, the native `WebView` is reached by
`WebViewFlutterAndroidExternalApi.getWebView` (as `snapshotWebView` does,
`MainActivity.kt:53`) and its client wrapped by a delegating
`WebViewClient` that answers the scheme's hosts and forwards everything
else to webview_flutter's client (`WebView.getWebViewClient()`, API 26;
minSdk is 29). On Apple a scheme handler must be set on the
`WKWebViewConfiguration` before the `WKWebView` exists, and
webview_flutter_wkwebview 3.27 has no hook for it: the plan carries a small
patch to the plugin (`bazel/flutter/plugin_overlays/webview_flutter_wkwebview/`,
and for the `-native` twins the same patch applied by `scripts/patch-pub.sh`
to a copy under `apps/clax_app/build-native/`), adding a static
registration hook the runner fills with Clax's handler. The alternative,
a platform view of the app's own for the shell, is recorded.

## 5. The shell in app mode

### 5.1 Calls

The shell makes its requests with `fetch` in about forty places
(`api.ts`, `threads.ts`, `caps/*`, `stream-hub.ts`, …). Rather than touch
each, app mode replaces the shell document's `fetch` once, at boot, with
the transport's (`web/shell/src/transport.ts`). When `AppState.embed` is
set the core writes into every shell document
`<meta name="clax-embed" content='{"transport":"host",…}'>` and right
after it `<script src="/_clax/transport.js">`, the transport built on its
own as a classic script, so it runs before the shell's modules and no
browser entry carries it. It replaces `fetch`, hides the connection
notices (§5.2), and installs `window.claxTransport {init, callResult,
streamBlock, streamEnd}`, where the app answers, in every shell document
whichever entry it loads:

- a request to the shell's own origin under `/api/` is posted on
  `ClaxHost` as

  ```json
  {"type":"call","nonce":"…","id":17,"method":"POST","path":"/api/…",
   "headers":[["content-type","application/json"]],
   "body":"<text>" | null,"body64":"<base64>" | null}
  ```

  and resolves to a real `Response` when the host calls
  `claxTransport.callResult(nonce, 17, {status, headers, body | body64})`
  with the nonce the call carried; an aborted
  `signal` rejects it with an `AbortError` and posts `{type: "callAbort",
  id}`. The host relays it as an `as: owner` frame;
- every other request goes to the platform's `fetch` (the scheme answers
  it, §4.2).

Every answer names the nonce of the load whose call it answers, and the
transport drops one naming another: IDs start at 1 in every document, so a
late answer to a WebView's previous load would otherwise land on the new
load's call or stream. Calls go out from boot under the document's nonce
(§4.4); a document without one waits for `claxTransport.init(nonce)`,
the way in for the app over loopback until its scheme handler writes the
nonce, and a call or stream whose signal aborts meanwhile rejects with an
`AbortError`, posting nothing. A body
over 8 MiB goes as `body64` in chunks of 1 MiB (`{type: "callChunk", id,
n, last, body64}`), so no one message carries more. `/api/token` answers as
on the daemon (the shell's own code may hold the token, as it does today;
no frame of a page can ask for it). Images and downloads the shell points
at by URL under `/api/` (clip PNGs, asset previews) are fetched with
`fetch` in app mode and shown by object URL; the plan's task lists each
such use by a grep.

### 5.2 Events

In app mode the stream hub (`stream-hub.ts`'s `Hub`) runs in the shell
document itself (no SharedWorker, no leader election), with the
transport's `fetch` as its `env.fetch`, which answers `GET /api/stream`
with a `Response` whose body is a `ReadableStream`: it posts `{type:
"subscribe", id, lastEventId?}`, `lastEventId` the request's
`Last-Event-ID` (or `last_event_id` query) when it names one, the host
opens a core subscription for that document from that point (§2.4) and
pushes each block of the core's subscription, the closing `event: end`
included (§2.4), by `claxTransport.streamBlock(nonce, id, text)`, which
the stream enqueues, then calls `claxTransport.streamEnd(nonce, id)`,
which ends the body. The hub's topic subscriptions go as today's
`POST /api/stream/<id>` calls.

In host mode the hub reads the `end` block's `{reason, status}` and
paces itself by it, as §2.4's consumers do:

- after `stream_refused` with 400, 403 or 404 it opens nothing, and opens
  the stream again only when a tab's topics change;
- after any other end (a body that ends with no `end` block included),
  and after a 5xx answer to `POST /api/stream/<id>`, it opens the stream
  again after the stream's backoff (`lifecycle.ts` `backoff(failures++)`:
  0.5 s doubling to 30 s), from its `Last-Event-ID`, so the new
  subscription resumes where the last one stopped; `ready` resets the
  count.

It has no connect, idle or subscription timeout, and no timeout on its
inbox requests either (the host answers every call), never requests
`/api/token`, and never tells the tabs `status` (no `stream_down`): a
silent stream is never failed on the host, which owns liveness. Its one
tab is never expired: the hub lives and dies with its document, so a
WebView whose timers stood still while it was suspended keeps its
topics.

The transport adds a style of its own that hides `.conn-notice`, so
"Live updates paused. Reconnecting…" and "Clax is not answering.
Retrying…" never show in the app; `conn-notice.ts` and `retrying()` are as
in a browser (a 5xx from the core is a real failure, retried).

A WebView that is hidden keeps its subscription (`setVisible` already keeps
its topics, `visibility.ts`).

### 5.3 What the browser keeps

The browser shell (no `clax-embed` meta) is unchanged: `fetch`, the SharedWorker hub over SSE, the notices.
`web/e2e` runs every existing spec against the daemon as before.

## 6. The app over the core

- **`CoreHost`** (`lib/core/core_host.dart`) replaces `DaemonSupervisor`: it
  opens the core at launch (§2.5) and exposes `state` (`Opening(phase)`,
  `Open`, `Failed(code, message, backup)`). There is no restart, no
  backoff, no endpoint, no PID. `onResume` does nothing.
- **`CoreClient`** (`lib/core/core_client.dart`) holds the handle and
  `call(Frame) → Future<Response>` for one isolate: a `ReceivePort` for
  responses, a tag counter, a map of pending completers. Each isolate makes
  its own; the handle is shared.
- **`ClaxTransport`** (`lib/clax/transport.dart`) is what `HttpClaxApi`
  sends through: `send(method, path, {query, headers, body, timeout}) →
  (status, bytes)`, `sse({resumeId}) → Stream<SseMessage>` (`GET
  /api/stream`, the API's one event stream: every reader in the app
  follows its topics, `/api/events` is not read) and `close()` (nothing
  over the core, whose lifetime is its host's).
  `HttpClaxApi` keeps every method and path (`lib/clax/api.dart`'s ~50) and
  its `_send` and `_sse` become calls on the transport. `CoreTransport`
  (`lib/core/core_transport.dart`) sends `as: owner` frames (an agent call
  keeps its `x-clax-session` header; the core adds the token) and makes
  `sse` a core subscription (§2.4) parsed by `sse.dart`'s block parser.
  `LoopbackTransport`, today's `HttpClient` code, lives until phase 7.
  Either transport says "no answer" one way: `send` throws `ClaxApiError`
  status 0 with code `timeout` (none came within the call's limit) or
  `unreachable` (none can come), the transport's own reason
  (`daemon_unreachable`, `core_closed`, `core_failed`, `frame_refused`,
  `malformed`) in `body.reason` and the message, and domain code asks
  `ClaxApiError.unanswered`, never the code. A stream that cannot go on
  ends with `ClaxApiError` on both: code `stream_refused` with the
  refusal's status, or status 0 `unreachable` with the core's reason.
- **`SseClaxStream`** (`lib/clax/stream.dart`) keeps its protocol logic and
  takes the core subscription as its `connect`; `SharedClaxStream`
  (`lib/clax/shared_stream.dart`), which existed to outlive a daemon
  restart, goes.
- **`FakeCore`** (`test/support/fake_core.dart`) answers frames from a
  `FakeClaxApi`-backed router in Dart and pushes events; every Dart test
  that used `fake_supervisor.dart` or `real_daemon.dart` uses it. The
  `*_real_test.dart` files run against the real core through the host
  build of `libclax_embed` (a `cdylib` for macOS built by Bazel for tests
  only, `//crates/clax-embed:clax_embed_host`), loaded with
  `DynamicLibrary.open` from `CLAX_EMBED_LIB`.
- **Loading the library:** Android `DynamicLibrary.open('libclax_embed.so')`;
  macOS `DynamicLibrary.open('<bundle>/Contents/Frameworks/libclax_embed.dylib')`;
  iOS `DynamicLibrary.process()` (the static library is linked into the
  runner with its four symbols kept and exported,
  `-Wl,-exported_symbol,_clax_embed_*`).

## 7. The queue

### 7.1 Shape

```
UI isolate                              worker isolate (lib/queue/)
──────────                              ─────────────────────────────
QueueClient ── Intent ────────────────► inbox ─► update(model, msg) ─► (model, effects)
  ▲  (make, change, cancel, retry,                     │                   │
  │   keepGoing, discard)                              │                   ▼
  └──────────── QueueState ◄─── PublishState           │           EffectRunner ── core calls ──► the core
RenderService ◄── RenderRequest ◄──────────────────────┼──────────── Render         (the only code
  └──── RenderResult ─────────────────► inbox           │                            that knows routes)
                                                        └─ msgs ◄─── answers, JobChanged events
```

- **Model** (`lib/queue/model.dart`): `QueueModel {owner, jobs: Map<JobId,
  Job>, inFlight: Map<EffectId, Effect>, local: Map<JobId, JobLocal>,
  pending: List<Intent>, claude: Reachability, canRender: bool, keepAlive:
  bool}`. `jobs` is a projection of the core's job rows (§7.4): `Job {id,
  kind: make|change, state, step, rev, artifactId?, title, input, plan?,
  carry?, error?, steps, threads, questions}`, each field as the core last
  sent it. `JobState` is the core's: `queued`, `running`, `waiting` (on the
  person's answer to a question), `paused` (the OS's time ran out),
  `stopped` (a limit; Keep going offered), `failed` (Retry offered),
  `cancelled`, `done`. `JobLocal {waitingFor: claude|render|null,
  renderTries, turn}` holds what is not durable and is rebuilt after a
  restart. `owner` is `none`, `claiming`, `held(token)` or `refused` (§7.5).
  `pending` keeps intents that arrive before the jobs are loaded.
- **Messages** (`lib/queue/msg.dart`): the intents; the owner's
  (`OwnerClaimed(token)`, `OwnerHeld`, `OwnerReleased`); the core's rows
  (`JobsLoaded(jobs)`, `JobChanged(job)`, and every commit effect's answer,
  `Committed(effect, job, {step, replayed})` or `Refused(effect, jobId,
  reason, {job})`, `reason` one of `versionMoved(current)`, `docMoved`,
  `notRenderedClean`, `jobMoved`, `notOwner`); work results (`TurnDone`,
  `TurnFailed(network|other)`, `ToolDone`, `Rendered`, `RenderLost`);
  `CanRender(bool)`; `ClaudeReachable(bool)`; `Tick(now)` from the injected
  clock; `Woke(foreground: bool)`.
- **`update`** (`lib/queue/update.dart`) is a pure function with no
  `async`, no I/O, no clock: time arrives in messages. It decides each
  job's next step from the job's row (§7.4) and emits effects.
- **Effects** (`lib/queue/effect.dart`) are domain operations; none names a
  path, a method or a frame:
  - *commit effects*, each answered by the core with the job's row:
    `CreateJob(id, jobId, kind, input, {artifactId, title, plan})`,
    `MoveJob(id, jobId, ifRev, to, {error, carry, plan, title})` (a change
    of state), `CommitStep(id, jobId, key, notes)` (a turn's notes),
    `Ask(id, jobId, question)` (the job waits on the person's answer),
    `Publish(id, jobId, base, publish)` (the publish step) and `Reply(id,
    jobId, reply)` (the reply step);
  - *work*: `StartTurn(id, jobId, turnInput)`, `RunTool(id, jobId, call)`,
    `Render(id, jobId, renderSpec)`, `ProbeClaude()`;
  - *the queue's own*: `ClaimOwner()`, `LoadJobs()`, `Notice(kind, jobId,
    title)`, `KeepAlive(on)`, `PublishState(state)`.
- **The effect runner** (`lib/queue/runner.dart`) runs each effect and
  feeds its result back as a message. It is the only code in the worker
  that knows the core's routes: it maps each commit effect to its route of
  §8.1 (with the owner token), each answer to `Committed` or `Refused`, and
  `Render` to a `RenderRequest` and then the render's record (§8.2). It
  holds the agent's runners and backends (`lib/agent`, unchanged in
  substance), a `CoreClient`, the `ApiConnection` probe, and the render
  port. It is the only code in the worker that awaits.
- **`update`'s tests assert meaning**: which job moves to which state,
  which step is committed with which base, that no second `Publish` is
  emitted for a base. They never assert a path, a method or a body's JSON;
  the runner's tests (against `FakeCore`) are where routes are checked.
- **Up to three jobs run at once**, as today (`make_service.dart`); others
  wait `queued`. A change of an app waits while a make or another change of
  the same app runs.

### 7.2 The UI isolate

`QueueClient` (`lib/queue/client.dart`) is all the UI knows of the queue: a
`ValueListenable<QueueState>` (an immutable snapshot of every job, built by
the worker after each `update` that changed it, jobs in flight to the core
included) and `send(Intent)`. It spawns the worker isolate at launch, once
the core is open, with the handle, the support path, the settings snapshot
and the key; settings and key changes are intents too. `MakeService`,
`FeedbackWorker`'s UI half, `ChangeTracker`, the launcher's tiles, the
making screen, the working chip, the change sheet and the thread page read
`QueueState` and send intents; none awaits a job.

If the worker isolate dies (an uncaught error), its ports close with it.
The client restarts it once; the new worker claims the queue's owner token
(§7.5), which the core releases when it notices the dead isolate's event
stream end, then loads the jobs from the core and resumes each (§7.4). A
second death in a minute shows "Clax's builder stopped. Restart Clax to go
on." on the affected jobs.

### 7.3 Rendering

A `Render` effect posts `RenderRequest {id, jobId, draftId, revision,
kind: inspect|tests|migration, settleMs, maxBytes}` to the UI isolate's
`RenderService`, which runs `WebViewInspector` (`lib/ui/inspector_surface.dart`)
on the shell's inspect view at the draft's URL (§4.1) and answers
`RenderResult {id, inspection | tests | migration | error}`. A request not
answered in 60 s on the worker's clock, or one whose WebView was torn down
(`RenderLost`), is sent again once, then the job waits for a render
(`waitingFor: render`, local) until the UI isolate says it can render
again. The runner records the render's result in the core (§8.2) before it
answers `Rendered`.

### 7.4 One source of truth, and resume

**The rule.** The core's job rows are the truth about every job. The
worker's `jobs` is a projection of them, rebuilt from `JobsLoaded` and
`JobChanged`, plus the effects in flight. `update` never advances a job's
durable state (its `state`, `step`, steps, `plan`, `carry`, `error`, threads
and questions) except by emitting a commit effect and applying the core's
answer, or the `JobChanged` event that answer causes. It applies a row only
when its `rev` is above the one it holds, so an answer and its event, in
either order, apply once. Everything else in the model (`local`, `inFlight`,
`pending`, `claude`, `canRender`) is not durable and is rebuilt after a
restart.

What follows from the rule:

- A kill loses only what was in flight. Each commit effect either landed in
  the core, and its row says so, or did not, and its row does not.
- Resume is not a separate path. After every applied row `update` asks
  `next(job)` for the job's next effect, the same function at `JobsLoaded`
  as after any `Committed`; the resume table is that function's table.
- A commit refused because the job moved (`job_moved`) carries the current
  row; `update` applies it and asks `next` again.
- The UI shows what the core holds: a job created by an intent shows as
  `queued` from its `CreateJob` in flight, and as the core's row once
  answered.

`next(job)`, at `JobsLoaded` (from `GET /api/jobs?open=1&client=app`) and
after every applied row:

| The core's row | `next(job)` |
|---|---|
| `queued` | `MoveJob(→ running)` when fewer than three run; else nothing |
| `running`, no step | `StartTurn(turn 1)` |
| `running`, last step `turn:<n>` with tool calls not yet answered | for each, in order: `finish` → `Publish` (base from the notes); `render` → `Render`; `ask` → `Ask`; any other → `RunTool`. A tool's result is not durable: after a restart the tools of the last committed turn run again (they are reads and draft writes, which repeat harmlessly) |
| `running`, last step `turn:<n>`, every call answered | `StartTurn(turn n + 1)` with the notes and `carry` |
| `running`, a `publish:<b>` step, a change | `Reply` (its step moves the job to `done` in the same transaction) |
| `waiting` (its question not closed) | nothing (the reaper spares the question's asker while the job is open, so a relaunch long after a kill finds it still asked, §8.1); the question's close (an answer, a decline or a withdrawal) stores its outcome and moves the job to `running` in the core (§8.1), and its `JobChanged` brings `next` again, which answers the `ask` call with that outcome |
| `paused` | `MoveJob(→ running)` at launch or `Woke(foreground: true)` |
| `stopped`, `failed` | nothing; `KeepGoing` and `Retry` emit `MoveJob(→ running)` |
| `cancelled`, `done` | not loaded; any result still in flight for it is dropped |

A make's publish step and a change's reply step carry `state: done`, so
the step and the end of the job commit together; there is no row in which
a make has published and is not done.

A step is idempotent by its key: committing a key the job already has
returns the recorded result with `replayed: true` and changes nothing
(§8.1). So a repeated commit (a worker that crashed after the core's
commit and before it saw the answer) can repeat a model turn, never a
publish or a reply. Two workers never run one job: only the owner (§7.5)
may commit.

### 7.5 The queue's owner

One worker runs the app's jobs at a time, chosen by the core, with no
clock:

- At its start the worker subscribes to the core's events (its own
  subscription, §2.4) and reads its stream's ID from the hub's `ready`
  block. `update` emits `ClaimOwner` once the core is open; the runner sends
  `POST /api/jobs/owner {client: "app", stream: <that ID>}`.
- The core keeps one claim per `client`, in memory only: `{token, stream}`.
  A claim answers 200 `{owner: <token>}` when there is none, and 409
  `owner_held` while the holder's stream lives. Every write to a job of that
  client (creating, moving, steps, questions, the import of §9) carries the
  token in `x-clax-jobs-owner`; a write without it, or with another,
  answers 409 `not_owner`. Reads need only the owner's credential, as today.
- The claim is released when the holder's event stream ends, on `DELETE
  /api/jobs/owner` with the token, or when the core closes; a reopened core
  starts with no claim. A worker isolate's stream ends when the isolate
  dies: Dart closes its ports, the core's next post to it fails (§2.2), and
  the subscription ends. A claim that finds a holder asks the hub to send
  the holder's stream a keep-alive at once, so a dead holder is noticed at
  that post rather than at the next periodic keep-alive.
- The core publishes `Event::JobsOwner {client, held}` on the `jobs` topic
  at each claim and release. A claim first subscribes the claimant's
  stream to `jobs` (400 `no_such_stream` when the stream is not the
  caller's), so a worker refused with `owner_held` hears the release it
  waits for (`held: false`), however soon it comes, and claims again. A
  worker refused a write with
  `not_owner` stops emitting work and claims again.
- The localhost daemon can serve the same API later unchanged: there an
  owner is any single caller holding a live `/api/stream` connection, and
  the connection's end releases its claim.

### 7.6 Background

`KeepAlive(on)` effects become `ChannelBackgroundWork.start/stop` on
Android (the `dataSync` foreground service, `BackgroundRunService.kt`), and
`IosBackgroundWork`'s background task on iOS, by a message to the UI
isolate (the platform channels live there). The progress notification is
built from `QueueState`. On Android the engine is kept by the application
(S8), so Back out of Clax does not end a job. On iOS and the Mac a job
running out of time is `paused` (a `MoveJob`). A job interrupted by the OS
or a kill on any platform resumes at the next launch (S6); nothing wakes
the app to run it.

## 8. The core: jobs and publish checks

### 8.1 Jobs and steps

Migration 46 (the next free number after main's 40 and this branch's five
app migrations, renumbered to 41 to 45 by §10.2; the number moves with
main's, as §10.2 says) adds:

```sql
CREATE TABLE jobs (
    id TEXT PRIMARY KEY,
    client TEXT NOT NULL,              -- 'app' (who runs it)
    kind TEXT NOT NULL,                -- 'make' | 'change' (opaque to the core)
    artifact_id TEXT REFERENCES artifacts(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('queued','running','waiting','paused','stopped','failed','cancelled','done')),
    step TEXT,                         -- the current step's key
    title TEXT,
    input_json TEXT NOT NULL CHECK (json_valid(input_json)),  -- the request, the threads and comments answered
    plan_json TEXT CHECK (json_valid(plan_json)),  -- a make's plan (was plans.json)
    carry_json TEXT CHECK (json_valid(carry_json)),  -- Keep going's carried draft and notes (was unfinished.json)
    error_json TEXT CHECK (json_valid(error_json)),
    rev INTEGER NOT NULL DEFAULT 1 CHECK (rev >= 1),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX jobs_open ON jobs(state) WHERE state NOT IN ('cancelled','done');
CREATE INDEX jobs_by_artifact ON jobs(artifact_id) WHERE artifact_id IS NOT NULL;
CREATE TABLE job_steps (
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    result_json TEXT NOT NULL CHECK (json_valid(result_json)),
    version_n INTEGER,                 -- the version a publish step made
    created_at TEXT NOT NULL,
    PRIMARY KEY (job_id, key)
);
CREATE TABLE job_threads (            -- was the ledger's answered and stopped.json
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    comment_id TEXT NOT NULL,          -- the newest comment the job answers
    stopped INTEGER NOT NULL DEFAULT 0 CHECK (stopped IN (0, 1)),
    PRIMARY KEY (job_id, thread_id)
);
CREATE INDEX job_threads_by_thread ON job_threads(thread_id);
CREATE TABLE job_questions (          -- was asked.json
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    question_id TEXT NOT NULL UNIQUE REFERENCES questions(id),
    outcome TEXT CHECK (outcome IN ('answered','declined','withdrawn')),  -- NULL until the question closes
    answers_json TEXT CHECK (json_valid(answers_json)),  -- the answers, as the question keeps them
    PRIMARY KEY (job_id, question_id),
    CHECK ((outcome IS 'answered') = (answers_json IS NOT NULL))
);
CREATE TABLE agent_read_consents (    -- was agent-read-consent.json
    reader TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
    source TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (reader, source)
);
CREATE INDEX agent_read_consents_by_source ON agent_read_consents(source);
CREATE TABLE draft_renders (          -- the render gate (§8.2)
    draft_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
    digest TEXT NOT NULL,
    clean INTEGER NOT NULL CHECK (clean IN (0, 1)),
    created_at TEXT NOT NULL,
    PRIMARY KEY (draft_id, digest)
);
CREATE TABLE client_imports (         -- one row per one-time import (§9)
    key TEXT PRIMARY KEY,
    done_at TEXT NOT NULL,
    summary_json TEXT NOT NULL CHECK (json_valid(summary_json))
);
```

Routes (owner credential only; `clax-server/src/routes/jobs.rs`; also on
the hosted Worker, which shares the router, though nothing there calls them
yet). Every route that writes a job of a client also needs that client's
queue-owner token in `x-clax-jobs-owner` (§7.5), else 409 `not_owner`;
each write answers with the job's row after it (`{"job": {…}}`), which is
what the worker applies:

| Route | Effect |
|---|---|
| `POST /api/jobs/owner` `{client, stream}` | claims the client's queue (§7.5): 200 `{owner}`; 409 `owner_held`; 400 `no_such_stream` when `stream` is not a live stream of the hub; a claim by the stream that holds the queue answers its token again |
| `DELETE /api/jobs/owner` | releases the claim the request's `x-clax-jobs-owner` token holds (204) |
| `POST /api/jobs` `{id, client, kind, artifact_id?, title?, input, plan?}` | creates (201); an existing `id` with the same body answers it (200), another body 409 `job_exists` |
| `GET /api/jobs?open=1&client=app` | open jobs with their steps, threads and questions |
| `GET /api/jobs/{id}` | one |
| `PATCH /api/jobs/{id}` `{if_rev, state?, step?, title?, plan?, carry?, error?}` | compare-and-set on `rev`; 409 `job_moved` with the current job; the state moves by the table below |
| `PUT /api/jobs/{id}/threads/{tid}` / `DELETE` | the job answers, or stops at, a thread's comment |
| `PUT /api/jobs/{id}/questions/{qid}` | a question the job asked; moves a `running` job to `waiting` in the same transaction, which reads the question: one that closed before this call (an answer landing between the question's creation and this call, or a worker re-asking after a kill) has its outcome stored at once, and the job keeps its state; a `cancelled` or `done` job refuses (409 `job_moved`). Only a question of the `ask` tool, which no other job asked (400 `invalid_question` otherwise). When the question closes (`POST /api/questions/{qid}/answer` or `/decline`, as today, or its withdrawal) the core stores its outcome here (`answered` with the answers, `declined`, `withdrawn`) and moves the job back to `running` in that close's transaction. A job's final move (a patch to `cancelled`, a `done` step, its artifact's delete) withdraws each question it asked that is still open, with the reason `job_end` (`Reason::JobEnd`, recorded as `system:daemon`): no worker reads its outcome again, so the person is no longer asked; the outcome `withdrawn` is stored and the job moves no further |
| `POST /api/jobs/{id}/steps/{key}` `{result?, publish?, reply?, state?}` (one of `result`, `publish`, `reply`) | commits a step (below); `state` (only `done`) ends the job in the step's transaction; answers `{job, step, replayed, version?}` |
| `GET/PUT/DELETE /api/agent-read-consents[/{reader}/{source}]` | the agent's remembered consents |

**The states.** One table in the store (`jobs::may_move`) says how a job
moves, and every write of a job obeys it; a job's `state` is a Rust enum
(`JobState`) and the SQL `CHECK` lists the same eight:

| From | By | To |
|---|---|---|
| `cancelled`, `done` | any write | refused: these are final (409 `job_moved` with the row); a step's replay still answers the recorded step |
| any other | `PATCH` to `running` | `waiting` while the job holds a question that is not closed, else `running` |
| any other | `PATCH` to `paused`, `stopped`, `failed` or `cancelled`, or naming the state it is in | that state |
| any other | `PATCH` to `queued`, `waiting` or `done` | refused, 400 `invalid_move`: only creation makes `queued`, only asking `waiting`, only a step `done` |
| `running` | `PUT …/questions/{qid}` | `waiting` while the question is not closed |
| any other | `PUT …/questions/{qid}` | unchanged; a move to `running` later lands in `waiting` |
| `waiting` | the close of a question it asked | `running` once it waits on none |
| any other | the close of a question it asked | unchanged |
| any other | a step | unchanged; `done` with `state: done` |

So a job is `waiting` exactly while it would run and holds an unclosed
question, and nothing writes a row that no sequence of these moves reaches.

**Deletes.** Every foreign key of these tables is `ON DELETE CASCADE`
(except `job_questions.question_id`: questions are never deleted), so a
hard delete of an artifact or a thread (a purge, a draft's delete, a
thread's delete) takes the rows naming it rather than failing. Each
delete path first ends what the cascade would hide from the worker:
`artifact_going_tx` moves the open jobs naming the artifact to
`cancelled` with the error `{"code": "artifact_deleted"}` (the soft
delete does this too), and `thread_going_tx` forgets the thread in each
job that answers it; each job so changed moves one `rev` on, and its
`Event::Job` follows the commit.

The owner claim is the core's, not the store's: `AppState.jobs_owners`
(`clax_http::jobs_owner::JobsOwners`) keeps a map (client → token and
stream ID) in memory, so a crash, a `close` or a reopen forgets it. The
token is 32 hex characters from the operating system's random source. The
hub tells it when a stream's connection ends (`Hub::closed(stream)`, a
future that resolves at the connection's end, a newer connection's
takeover or the stream's drop, and at once when no connection holds it),
and `Hub::nudge(stream)` has that connection send a keep-alive at once.
A claim whose holder's stream no longer has a connection replaces it.
Nothing in the store knows owners, and nothing in the claim reads a
clock. Job routes take the owner credential (the token) and refuse the
owner's agents (403 `not_for_agents`). Each write is admitted under the
claim (`JobsOwners::enter(client, token)`, which counts the claim's
writes in flight under the map's mutex) and holds the guard it answers
across its store call, the async publish step's included. A release, or
a claim that replaces one whose stream is gone, takes the claim out of
the map at once, so no write is admitted under the old token from then
on, and the new claim answers only once the writes admitted under the
old one have ended (it waits on a `Notify` each guard's drop signals).
So no old owner's write lands after the new owner holds the queue.

**A publish step** carries `publish: {artifact_id | null (a new app),
base, request: PublishRequest, docs: [{op, path, data?, if_version?}],
draft?: {id, digest}, session_id?}`. `Store::commit_publish_step` first
answers a key the job already has with its step as recorded and
`replayed: true`, writing nothing, and refuses a final job (409
`job_moved`). Else it writes the content files as `publish_version` does
(under the artifact's content lock, once the artifact is seen at `base`)
and then, in the version's one `IMMEDIATE` transaction: checks
`current_version == base` (409 `version_moved` with `current` otherwise,
nothing recorded), inserts the version (or the artifact), applies the
pinned document writes after the artifact's row, so they see the
capabilities the version declares (a moved document, or an existing one
written without a pin, refuses the whole step, 409 `doc_moved` with its
`path`, as `finish` refuses today), inserts the step row with `version_n`
and the result `{artifact_id, version, docs}`, and moves the job one
`rev` on with `step` set to the key, `artifact_id` to a new app's, and
`state` to `done` when the body says so. A transaction that fails removes
the content it wrote, and a new app's zero-version row. Events
(`Event::Version`, the docs events, `Event::Job`) are published after the
commit. A new app's request needs a title, as `POST /api/artifacts`
does. With `session_id` (checked as `X-Clax-Session` is:
`publishing_session`) the step does for the session what the publish
routes do, through the same function (`routes::artifacts::published_from`):
the threads the session was working on are linked to the version
(`working_threads`), its working records on the artifact end
(`End::Publish`), its draft of the artifact ends, it watches the
artifact (`ensure_watch`), and the artifact's untargeted feedback is
targeted to it (`retarget_untargeted`). A replayed key does none of it.
A **reply step** (`reply: {session_id, threads, body, resolve?,
ack?}`) makes, in one transaction the same way, the session's agent reply
on each thread (each a thread of the job's artifact, 400 `invalid_thread`
otherwise), each thread's resolve when `resolve`, the acknowledgement of
the session's feedback on those threads and on the comments `ack`, and
the step (`{comments: [{thread_id, comment_id}], resolved}`); the
working records' clear, which is in memory, and the comment and thread
events follow the commit.

A change's reply step is made from the session that published its
version, which may be resumed long after the app was killed. So the
core's reaper (`Store::reap_sessions`, which ends a local session unseen
for `SESSION_IDLE_SECS`) leaves live every session that published a
version one of an open job's steps recorded, and the asker of each
question an open job asked that is not closed (`OPEN_JOB_SESSIONS`): the
reply step of a change relaunched after more than five minutes commits,
and a `waiting` job's question stays in front of the person, however
long the app was down, until it is answered or declined. Once the job is
`done` or `cancelled`, or the question closes, the session is reaped as
any other. An end of the asker for any other reason (it ends itself, its grant
is revoked) still withdraws the question and moves the job back to
`running`, and `next` answers the `ask` call with `withdrawn`: nothing
asks again unless the model chooses to.

A process killed after the content files are written and before the
transaction commits leaves the files at the next version's number with
no row; they are never served. The step's write marks them with the step
(`clax_store::jobs::step_mark`: the job ID, the key and a nonce, as a
JSON array). The nonce is 32 hex characters from the operating system's
random source, minted by the store at the step's first write and kept in
migration 47's `job_step_marks` (nonce, job, key, the artifact and
version it writes, the session it publishes from); the row goes when the
step commits or its content is removed, and has no foreign key, so it
outlives its job. Git's version commit carries the mark as a
`Clax-Step` trailer, the home's files as `versions/<n>.step` beside the
version's directory, written before the rename that places it. The store
decides what a mark is, never the content: a mark is a step's only when
`job_step_marks` holds its nonce for that job, key, artifact and version
(`jobs::mark_of`); any other, a job ID and key guessed or a whole mark
copied into a pushed commit's message, is no mark. The resumed step,
under the content lock and with the artifact still at `base`, finds `n`
present with its own mark (`ContentStore::unrecorded_step`), removes it
and writes it again, then commits once; git's push reconcile
(`ContentStore::pushed_commits` lists each tagged commit's tag and mark,
`PushedCommit::marked`) leaves a tagged commit with a step's mark to
that step rather than recording it as a push, and records one whose
mark the store does not hold as the push it is. Content no step marked,
or another step marked, is never replaced: the files refuse the write as
before (`clax doctor` lists a stray directory), and git records an
unmarked commit as a push.

So the change ledger's two unknowns, "did the publish land?" and "did the
docs land?", cannot arise: either the step row and the version exist
together, or neither does.

Events, on a new stream topic `jobs` (owner only), which the worker
subscribes to: `Event::Job {id, client, state, step, rev}` after every
committed write of a job, whichever path made it (a job route, a
question's close, a delete that cancels the job): the store's update
hook records every written `jobs` row and `Store::set_job_listener`'s
listener hears each changed job once per committed transaction, which
the router publishes, and `Event::JobsOwner {client, held}` at each
claim and release.

### 8.2 Publish checks

`clax_core::publish::validate` (`crates/clax-core/src/publish.rs:260`) gains,
for every publish from any client (the app, an outside agent over MCP or
the CLI, a push, the hosted Worker):

- **icon.svg** (`clax_core::icon::problem(&[u8]) -> Option<IconProblem>`,
  a port of `apps/clax_app/lib/agent/icon_svg.dart`'s `iconProblem`): at
  most 32 KB; well-formed XML (parsed with `roxmltree` 0.20, a new dependency of
  `clax-core`: no unsafe, no I/O, builds for wasm32; `just bazel-sync`
  after adding it); no DOCTYPE; root
  `<svg>` with a square four-number `viewBox`; elements only from
  `iconElements`; no `on*` attribute; `href` only to `#` references in the
  file; no `filter`; every `url()` a `#local` one; `<use>` expanding to at
  most 64 copies and nesting at most 3 deep. A bad icon refuses the
  publish with 400 `invalid_icon` and the rule broken. The Dart
  `iconProblem` stays as the agent's early warning, and a shared fixture
  (`crates/clax-core/tests/fixtures/icons/*.svg` with `expect.json`) is run
  by both the Rust and the Dart tests, so the two cannot drift.
- **The page rules**: `index.html` present and a title (already there,
  `require_title`), now with the same wording `finish` uses.
- **The render gate (S5)**: `POST /api/drafts/{id}/renders` `{digest,
  errors, failed_tests}` records a render of a draft's content, where
  `digest` is the SHA-256 over the draft's files but `icon.svg` (the
  content hash of `lib/bundle/content_hash.dart`, moved to
  `clax_core::digest::files_digest`). A publish step with `draft` is
  refused with 409 `not_rendered_clean` unless the draft has a render with
  that digest, no errors and no failing tests, and the request's files
  have that digest. A publish without `draft` (an outside agent, a
  restore, an update from a bundle) has no render gate, as today.

Outside agents' publishes that broke an icon rule before now fail. Their
skill copies (`plugins/*/skills/clax`, `docs/contract.md` "Page contract")
gain the icon rules in the same commit, and `scripts/sync-app-prompt.sh`
carries them to the app's prompt.

## 9. The JSON state, imported once

| File | Becomes |
|---|---|
| `pending-make.json` | a `make` job per request, `queued`, or `waiting` for those in `waiting` |
| `change-ledger.json` | a `change` job per record with a `publish:<base>` step when version `base + 1` exists and names the record's `addresses` (the ledger's own recovery rule), and a `reply` step when its `docs_done` and reply are found on the threads; else the job resumes before the publish |
| `plans.json` | `plan_json` of the make job whose request matches, or a `stopped` make job holding the plan (Resume build) |
| `asked.json` | `job_questions` rows of a `change` job per entry, `waiting` on the answer |
| `stopped.json` | `job_threads` rows with `stopped = 1` on a `cancelled` change job per thread |
| `unfinished.json` | `carry_json` of a `stopped` change job per entry (Keep going) |
| `agent-read-consent.json` | `agent_read_consents` rows |

`lib/core/json_import.dart` runs in the worker isolate once, at its first
start after this build, before it loads jobs:

1. if the core's `client_imports` has `app-json-v1`, do nothing;
2. copy the seven files, as they are, to `<support>/imported-json/<time>/`;
3. parse each with today's readers (`PendingMake`, `FileChangeLedger`,
   `PlanStore`, `FileAskedQuestions`, `FileStoppedThreads`,
   `FileUnfinishedChanges`, `FileConsentMemory`, kept for this and deleted in
   the last phase); a file that does not parse is skipped and named in the
   summary;
4. write the jobs and rows in one `POST /api/client-imports/app-json-v1`
   `{jobs, consents}`, carrying the queue's owner token (the worker claims
   the queue before it imports, §7.5), that the core applies in one
   transaction with the
   `client_imports` row (a crash before it leaves nothing; after it, the
   key);
5. delete the seven files.

Job IDs from the import are derived from the file's content (SHA-256 of the
entry), so an import that a crash interrupted between 4 and 5 and then
retried changes nothing.

## 10. Existing installs

### 10.1 What is on the owner's devices

The phone's and the Mac's homes are `clax-home/` under the app's support
directory, written by the `clax` the app ran, at this branch's migration
43 (`APP_MIGRATION_MARKS`, migrations 39–43), plus the seven JSON files.
The new build opens the same directory in place.

### 10.2 Against the renumbering failure

The failure of 2026-10-15 (a rebase renumbered migrations an installed
database already had, and the daemon re-ran them and crash-looped) is
designed against so. The owner's decision 13 settles the numbering: this
branch follows main's, as AGENTS.md requires (a branch takes the next free
number on main), and installed homes are adopted by their schema.

- **The numbering.** Main's `crates/clax-store/src/migrations.rs` ends at
  40 today (39: `legal_agreements`, a person agreeing to a hosted Clax's
  terms; 40: `briefs`, artifacts started from a brief), both plain
  `CREATE TABLE`s with no Rust step. This branch's list ends at 43, its
  39 to 43 being the app's five (`db_read_grants`, `artifacts.draft_of`,
  `net_grants`, `sample_grants`, `artifacts.draft_inherit_net`). Before
  migration 46 is written, the branch takes main's 39 and 40 verbatim
  (SQL and comments, character for character, so the merge of main
  resolves `migrations.rs` by their shared prefix) and moves the app's five
  to 41 to 45 (`APP_MIGRATION = 41`); the jobs migration is 46. If main has
  gained migrations when that task runs, it takes them all the same way
  and every number after them moves up by as many.
- **Installed homes at this branch's old numbering are adopted, never
  re-run.** The owner's phone and Mac are at `user_version` 43 with this
  branch's 39 to 43: read under the new numbering, 43 would claim main's
  39 and 40 and the app's 41 to 43, and the open would run the app's 44
  and 45, whose tables the home already has, and fail on every launch,
  which is the failure of 2026-10-15. So `adopt_earlier_flutter_app` (which
  already adopts the app's layouts that started at 24, 27 and 38) learns
  the layout that started at 39, by schema, not by `user_version`: the
  home has the app's first mark (`db_read_grants`) and the where-to-ask
  migration's `questions.thread_id`, and holds the first k of the marks of
  main's migrations between the where-to-ask migration and the app's
  (`legal_agreements`, `briefs`, in order) and none after them, with k
  below their number: the app's migrations in that home start at
  38 + k + 1 (39 for the installed homes, k = 0). Its version must be
  38 + k plus the number of the app's marks it holds, and those marks
  (the app's five, then the jobs migration's tables) must be present
  exactly up to that one; else, or when the main marks it holds are not
  the first k, the open is refused with `MigrationFailed` naming the
  mismatch and the backup, and the home is left as it was. An adopted
  home runs main's migrations it lacks (from 38 + k + 1 up to the app's
  first, which touch none of the app's tables), and its `user_version`
  becomes the number its last app migration has now (43 → 45). A home
  holding all of main's marks is of the new numbering and, like any other
  database, is left to the ordinary migrations.
- **Each main migration below the app's has a mark** (`MAIN_MARKS_ABOVE_38`
  beside `APP_MIGRATION_MARKS`: `legal_agreements`, `briefs`), so adoption
  decides from the database which of them a home lacks, whatever its
  version. A mark (`Mark`) is what the migration leaves: a table it makes,
  a column it adds to any table, an index it makes, or, for a migration
  that changes only rows, its marker (`Mark::Marker(name)`): a backfill
  that may change nothing cannot otherwise be told apart from its absence.
  A data-only migration's SQL starts with `migration_marker!("<name>")`,
  which makes the dedicated table `migration_marks (name TEXT PRIMARY
  KEY)` where no earlier data-only migration did (`CREATE TABLE IF NOT
  EXISTS`, so it needs no migration of its own and takes no number), with
  triggers that refuse an `UPDATE` or `DELETE` of it, and inserts its
  name. Nothing else writes the table, so a marker, unlike a row of
  `install` or of a user table, stays whatever the running store or the
  person does. Each mark must be absent before its migration and present
  after it and after every later one, and stay for as long as the
  database, which a store test checks for every main mark from an empty
  database. The rule above holds for any k, so a later
  renumbering adds one mark per new main migration and nothing else: when
  main adds a 41,
  a home this build left at 45 (main's two marks, k = 2, the app's from
  41) is adopted the same way, gets main's 41 after a backup, and its
  `user_version` becomes 46, never a refusal. Store tests run that case
  against a simulated main 41 of each kind of mark, and adopt the app's
  migrations after every k of main's under this build's own numbering,
  from a frozen copy of the app's SQL
  (`crates/clax-store/tests/fixtures/old-migrations/flutter-app-4479a586c.sql`),
  so this build's layout (the app's at 41 to 45) stays tested once main
  adds a 41.
- **Backup first, always.** `Store::open` already backs up before any
  migration (`backup::before_migrate`, three kept), adoption included:
  the first open of an installed home leaves `backups/clax-43-<time>.db`
  before anything is written.
- **The jobs migration's tables are adopted by schema too.** Its `jobs`
  table joins `APP_MIGRATION_MARKS` as the sixth mark, and adoption checks
  its other six tables (`job_steps`, `job_threads`, `job_questions`,
  `agent_read_consents`, `draft_renders`, `client_imports`) with it, so a
  home made by a build whose jobs migration had another number is
  recognised the same way, and one whose tables disagree with the marks is
  refused, not migrated over.
- **Fixtures of the installed schema.** Two, both made before the
  renumbering: `crates/clax-store/tests/fixtures/old-migrations/flutter-app-4508df19f.sql`,
  this branch's migrations 39 to 43 as commit 4508df19f has them (first:
  39, user_version: 43), in the format of that directory's other fixtures,
  which the store's adoption tests replay; and
  `crates/clax-embed/tests/fixtures/homes/flutter-app-43.sql`, the dump of
  a home written by this branch's own `clax` at schema 43, with a few rows
  of every kind, which a test opens through the core: it backs up,
  adopts, migrates to 46, and keeps every artifact, version, thread and
  document.
- **A large old database.** The test home of `scripts/perf-daemon.sh`
  (300 artifacts, 2,400 threads, 7,200 comments) is opened by the core in a
  test: main's 39 and 40 and the jobs migration add tables only (no
  rewrite of existing rows), so the open takes the time of their
  `CREATE`s. The open's progress messages keep the splash honest meanwhile.
- **Before installing on the owner's devices**, the plan's last task copies
  the phone's home off the phone (`adb exec-out run-as dev.empathic.clax
  tar …`) and the Mac's from its container, opens each copy with the new
  core on the Mac, and checks the counts, before `just app-phone` runs. No
  build carrying the renumbered or new migrations is installed on the
  owner's devices before that.

### 10.3 The JSON files

§9. The files are kept, copied, under `imported-json/` until the person
deletes the app; nothing reads them after the import.

### 10.4 Leftovers

At the first open the app deletes `<support>/daemon-port`; the core renames
`daemon.json` (§2.5). The WebViews' `clax_key_<port>` cookies are cleared
once (`WebViewCookieManager.clearCookies`), and each origin's storage under
`http://127.0.0.1:<port>` and `http://<id>.localhost:<port>` is left to the
WebView. A page's `localStorage` on the old origin is not carried to the
new one; the old port was kept across starts (`<support>/daemon-port`), so
this is a real, one-time loss for pages that keep state there rather than
in their documents. This spec does not carry it (copying it would need
each old origin loaded once over a loopback listener, which decision 1
forbids); whether that loss is acceptable is the owner's call (ask).

## 11. Build

| | Before | After |
|---|---|---|
| Android | `//crates/clax-cli:clax_android_arm64`, `:clax_android_x86_64` → `<abi>/libclax.so` (an executable named as a library) | `//crates/clax-embed:clax_embed_android_arm64`, `:clax_embed_android_x86_64` → `<abi>/libclax_embed.so`, a `rust_shared_library` built with the NDK by a rule beside `bazel/android/clax.bzl` (renamed `embed.bzl`); `extractNativeLibs` no longer needed (a library is loaded, not exec'd), so it goes with `useLegacyPackaging` |
| macOS | `macos_helper(":macos_clax", //crates/clax-cli:clax_opt)` in `Contents/Resources`, signed with `ClaxHelper.entitlements` | `//crates/clax-embed:clax_embed_macos` (`rust_shared_library`, `-c opt`, the host's arch) as `Contents/Frameworks/libclax_embed.dylib` through `flutter_macos_app`'s `additional_contents` (as rules_flutter places `App.framework`), signed ad hoc by the rule that signed the helper, without entitlements; no helper process |
| iOS | `//crates/clax-ios:clax_ios_c` through the `ClaxDaemon` Swift module | `//crates/clax-embed:clax_embed_ios_c` (the same `ios_rust.bzl` rules) as a `cc_library` dependency of the runner; no Swift module calls it (Dart does, by FFI) |
| Native twins | `scripts/build-app-clax.sh` (Cargo) | the same script builds `clax-embed` for the same places |

What the library carries: `clax-server` and below, the web UI (`clax-embedded`'s
`web` module), no `clax-commands`, no CLI, no plugins or extension trees,
no `clap`. The APK's and the macOS app's size is measured before and after
by `:apk_test` and `:macos_app_test` (they print each native file's size)
and reported in the phase 3 commit; the expectation is smaller per ABI
than today's `libclax.so`, which carries the CLI and the plugin trees, but
no number is promised.

`:apk_test`, `:macos_app_test` and `:ios_app_test` check the new contents:
the `.so` per ABI with its four exported symbols (`llvm-nm -D`), the Mac's
`libclax_embed.dylib` and the iOS runner with theirs (`nm -gU`), and no `clax`
executable anywhere.

## 12. Time to usable

A cold launch to a usable home screen must not regress; with no process
spawn, no port wait and no health probe it should improve.

- **The measure** is today's: `home-usable` (launch → the home app's first
  `shown()`) and `home-cover` in `timings.jsonl` (`docs/usage.md`,
  "Measuring the home screen's start").
- **`scripts/perf-app-launch.sh`** (new, run by `just perf` on a Mac and by
  hand with `--android` on a plugged-in phone; it gates nothing) cold-starts
  the app N times (default 9; `CLAX_PERF_QUICK=1`: 5) on a scratch
  `CLAX_APP_DIR` (macOS) or after `am force-stop` (Android), reads the new
  `timings.jsonl` lines, and writes the median and p90 of both measures to
  `CLAX_PERF_JSON`, beside a budget in `web/perf/budget.json`
  (`app_home_usable_ms`), so `scripts/perf-log.sh` keeps the trend.
- **The baseline** is measured on the current build (the daemon) before
  phase 3 lands, on the Mac and the owner's phone, and recorded in the perf
  log; phase 3's commit reports the after.
- **What should improve**, and why: the supervisor's spawn, the port file,
  `daemon.json` polling and `/healthz` (today the home WebView waits for
  `Up`) are gone; the home page's first request waits only for the store's
  open, which the core starts before the first frame.

## 13. What this deletes and supersedes

### 13.1 Files deleted (in the last phase, after nothing uses them)

- `apps/clax_app/lib/daemon/`: `supervisor.dart`, `health.dart`,
  `ports.dart`, `process.dart`, `in_process.dart`, `exe_locator.dart`,
  `sample_key.dart`, `daemon_key.dart` (`clock.dart` moves to
  `lib/platform/clock.dart`).
- `apps/clax_app/lib/ui/daemon_cookie.dart`; `lib/clax/shared_stream.dart`;
  `LoopbackTransport` and `reconnectingSse`'s HTTP code in `lib/clax/sse.dart`
  (its block parser stays).
- `apps/clax_app/lib/platform/pending_make.dart`, `lib/agent/ledger.dart`,
  `lib/agent/stops.dart`, `lib/agent/unfinished.dart`'s file store,
  `lib/agent/asked.dart`'s file store, `PlanStore`'s file, `FileConsentMemory`
  (after the import code, which keeps private copies of their readers, is
  the only reader).
- Tests: `test/daemon/` (supervisor, supervisor_real, in_process,
  sample_key, daemon_key), `test/ui/daemon_cookie_test.dart`,
  `test/support/fake_supervisor.dart`, `test/support/real_daemon.dart`,
  `integration_test/locked_daemon_test.dart`.
- `crates/clax-ios/` whole; `apps/clax_app/ios/Daemon/ClaxDaemon.swift`;
  the `daemon*` methods of `ios/Runner/NativeChannel.swift`; ATS's
  loopback exceptions in `ios/Runner/Info.plist`.
- Android: `nativeLibraryDir` in `MainActivity.kt`,
  `res/xml/network_security_config.xml` and its manifest reference,
  `extractNativeLibs`; `//crates/clax-cli:clax_android_*` and
  `bazel/android/clax.bzl` (replaced by `embed.bzl`).
- macOS: `macos_helper` and `:macos_clax` in `apps/clax_app/BUILD.bazel`,
  `bazel/flutter/macos.bzl`'s helper signing, `macos/Runner/ClaxHelper.entitlements`,
  the Xcode project's "Embed clax" phase.
- AGENTS.md's clax-ios exception line, replaced (§14).

### 13.2 Specs and plans superseded, in part

- App spec §3.1 (DaemonSupervisor) and its process diagram; round-2 spec's
  recovery by ledger (§ on `ChangeInterrupted` and the ledger); iOS spec
  §1–§2 (the in-process daemon, `clax_start`…`clax_trim_memory`, the key
  slot over HTTP, the supervisor's in-process starter), its Review Focus 1
  and 3; bazel.md's rows for `clax_android_*`, `macos_app`'s helper and
  `clax_ios_c`; `docs/usage.md` "The Clax app" first paragraph and "On an
  Android phone"'s port paragraph; `docs/contract.md` "The Clax app"'s
  first paragraph and the Security model's key-cookie part.
- Each superseded document gets one line at its top naming this spec and
  the sections it replaces; the text below stays as history.

### 13.3 Records this work closes or obsoletes (proposed to the owner)

`69557070` (a key change restarts the daemon), `9fbb9e03` (the 15 s health
limit), `c58da88f` (a key put not retried), `3f5fb92f` (a waiting make
reads "Offline"; §15 rewords it), `2713e7df` (the pill says Working while
waiting). Each is proposed for closing with the commit that removes its
code, never closed by this work's agents (AGENTS.md).

## 14. The `unsafe` exception

AGENTS.md's exception line becomes:

> - One exception, approved by the owner: `crates/clax-embed` denies `unsafe_code` instead of forbidding it, and allows it only in `src/ffi.rs`, on its four exported C functions (`clax_embed_open`, `clax_embed_call`, `clax_embed_subscribe`, `clax_embed_close`) and the one `std::slice::from_raw_parts` each of `clax_embed_open` and `clax_embed_call` makes of its byte arguments; `//scripts:no_unsafe` fails on any other `unsafe`, any other `allow(unsafe_code)`, and any other crate that does not take the workspace's lints.

`crates/clax-embed/Cargo.toml` forbids `unsafe_op_in_unsafe_fn` (edition
2024 only warns), so an unsafe operation in the body of an `unsafe fn`
export needs an `unsafe` block of its own, and the check, which reads
words, sees it; the check fails when the lint is not forbidden.
`scripts/check-no-unsafe.sh` allows, in `crates/clax-embed/src/ffi.rs` only:

1. `#[unsafe(no_mangle)]` directly after `#[allow(unsafe_code)]` and
   directly before `pub extern "C" fn <name>(` or
   `pub unsafe extern "C" fn <name>(`, `<name>` one of the four, each once;
2. `unsafe` in that `pub unsafe extern "C" fn` line, before its `(`, for
   `clax_embed_open` and `clax_embed_call` only (they take pointers); the
   rest of a signature line (its parameters, and a body on the same line)
   is checked as any other line;
3. inside those two functions' bodies, exactly one line
   `let <a> = unsafe { std::slice::from_raw_parts(<p>, <n>) };` directly
   after a `// SAFETY:` comment line;
4. parameters only of the types `u64`, `i64`, `i32`, `usize`,
   `*const u8` and `ClaxPost`.

Anything else (an `unsafe` elsewhere in the crate, a fifth export, a
`*mut` parameter, `Box::from_raw`, a second `from_raw_parts`) fails with
`only the four exported functions may use unsafe`. `crates/clax-ios` and
its rule leave the script and `scripts/test-check-no-unsafe.sh`, whose
cases are rewritten for clax-embed.

## 15. Words the person sees

- Local work shows no transport state anywhere: no "Clax is starting",
  "Clax is not answering", "Reconnecting" for the core. The core's open
  shows the splash mark (UI spec §5, breathing after 600 ms) and, if it
  fails, the migration-failure screen.
- "Waiting for connection" becomes **"Waiting for Claude"** everywhere it
  appears (`making_screen.dart:105`, `change_tracker.dart:256`,
  `thread_detail.dart:917`, the working chip, `home.js:279`'s spoken text).
- The home tile's "Offline" becomes **"Can't reach Claude"**
  (`home.js:267`, `launcher.dart:730`, `split/sidebar.dart:428`), keeping
  the `cloud_off_fill` glyph.
- A job paused by the OS reads "Paused — open Clax to keep building" (the
  iOS spec's words).

## 16. Testing

All tests stay fast and load-tolerant: no sleeps for a condition, no
wall-clock budgets, injected clocks (AGENTS.md).

- **`update` is pure and tested exhaustively**
  (`test/queue/update_test.dart`): a table over every `JobState` × every
  `Msg` kind asserting the next state and the effects, plus properties run
  over generated message sequences (a seeded `Random`, 2,000 sequences of
  up to 40 messages, the core's answers generated from the commit effects
  emitted): no job runs two `Publish` effects for one base; a `cancelled`
  or `done` job emits nothing; at most three jobs `running`;
  `KeepAlive(true)` exactly while some job is `running` or `waiting`;
  every `CommitStep` key is unique per job; a job's durable fields change
  only on a message carrying a core row (§7.4); no job effect is emitted
  while the worker does not hold the owner token. The tests assert
  meaning (states, steps, bases, effect kinds), never a route, a method
  or a body.
- **The effect runner** against `FakeCore` and the scripted runner
  (`test/queue/runner_test.dart`), on a fake clock: this is where each
  effect's route, the owner header and each answer's mapping are checked.
- **A worker isolate dies and restarts** (`test/queue/client_test.dart`):
  the dead worker's stream ends, the new worker's claim succeeds after the
  release, it loads the jobs from the core and resumes each by §7.4's
  table, and no step is committed twice.
- **A fake core for Dart tests**: `test/support/fake_core.dart` (§6).
- **Rust tests for clax-embed's `call` against the Router**
  (`crates/clax-embed/tests/`): an owner call lists artifacts; a content
  frame reads `/v/1/` of its artifact and gets the content policy with the
  shell origin in `frame-ancestors`; a content frame for `/api/artifacts`
  is refused; a `shell-page` frame for `/api/` is refused; `/api/stream`
  over `call` is refused; a long poll (`feedback?wait=2s` on a paused Tokio
  clock) answers when a comment arrives; a 97 MiB response answers 502;
  calls before `ready` wait; `close` ends them; a second `open` of the same
  home is refused; the subscription posts `ready` then a version event as
  an SSE block; the frame codec round-trips. The FFI layer is tested by a
  Rust test that calls the four `extern "C"` functions with a test `post`
  function collecting messages (safe: the test passes slices it owns).
- **Jobs and steps** (`crates/clax-store` unit tests and
  `crates/clax-server/tests/api_jobs.rs`): a publish step twice publishes
  once (`replayed: true`); a step on a moved base records nothing; a
  failure injected after the version insert and before the commit leaves
  neither; a moved document refuses the whole step; `if_rev` races; a
  second claimant of a client's queue is refused while the first's stream
  lives and takes it once that stream ends; a write without the owner's
  token is refused; a closed and reopened core holds no claim; the import
  route applies all or nothing.
- **Publish checks**: the icon fixtures in Rust and Dart; an outside
  agent's publish over MCP with a bad icon is refused; the render gate.
- **The shell's app-mode transport in `web/e2e`**
  (`web/e2e/app-transport.spec.ts`): Playwright's `page.route` answers
  `https://app.clax.invalid/**` and `https://*.c.clax.invalid/**` from a
  real test daemon with the frame rules of §2.3 (a small Node relay in
  `web/e2e/app-scheme.ts`, stripping `Access-Control-*`), and the fake
  `ClaxHost` relays `call` messages to the daemon with its token and pushes
  `/api/stream` blocks by `claxTransport.streamBlock`. It checks: the artifact
  view loads and comments post through the channel; a version published
  elsewhere is pushed and followed; the shell never shows a connection
  notice, also with the relay paused for 60 s on Playwright's clock; a
  content frame's `fetch` of the shell's `/a/<id>` and `/api/artifacts`
  reads nothing; a content frame cannot post to `ClaxHost` (the fake host
  drops non-main-frame messages, as the real ones do).
- **Two claimants of the owner token** (`crates/clax-embed/tests/` and
  `crates/clax-server/tests/api_jobs.rs`): the second is refused while the
  first's subscription lives; when the first's subscriber returns false
  (a closed Dart port), the claim is released, `Event::JobsOwner {held:
  false}` is heard and the second claim succeeds; a write with the first's
  token after that is refused.
- **Kill and resume**: `integration_test/kill_resume_test.dart` (macOS,
  `just app-integration`): a scripted make is killed (the test runs the app
  as a child and `kill -9`s it by its PID) right after the publish step's
  commit, then relaunched; it checks one version, the job `done`, the reply
  once. A second case kills before the commit and checks one version after
  resume. On Android the same scenario is a step of the plan's device
  checklist, run on the Android emulator (`adb shell am kill` while the
  foreground service runs, then launch).
- **The scheme spike** (§4.3) is an integration test on each platform:
  `just app-integration` on the Mac, and on an Android emulator under
  Bazel (`//apps/clax_app:integration_android_scheme_spike`), each
  asserting every value it reads.
- **Old homes**: the fixture test (§10.2) and the perf home open.

## 17. Phases

Each phase leaves main working and ships something testable.

1. **clax-embed with `call` and events.** The crate, `Core` split out of
   `serve`, frames, subscriptions, the FFI, the unsafe check, Bazel for all
   three platforms. Nothing in the app uses it yet. Ships: Rust tests, the
   library in each build's outputs.
2. **The shell's app-mode transport and the scheme.** `transport.ts`,
   host-pushed events, notices off in app mode, the scheme handlers and
   channel restrictions, `CoreClient`, `ClaxTransport` and `CoreTransport`,
   `FakeCore`. Still off in the app (a `--dart-define=CLAX_CORE=embed`
   switch for testing). Ships: e2e app-transport specs, Dart tests, the
   spike's results.
3. **All three platforms off the daemon.** `CoreHost` replaces the
   supervisor; the scheme replaces the port; the APK, the Mac app and the
   iOS app carry the library and no `clax`. Ships: the app on the core,
   the time-to-usable before/after, old-home adoption.
4. **The core's jobs and publish checks.** The migrations on main's
   numbering with the adoption of installed homes (§10.2), the jobs
   migration, the job routes with the queue owner, the publish and reply
   steps, the icon, page and render checks. Nothing in the app calls the
   job routes yet. Ships: the Rust tests, old homes adopted.
5. **The queue in the worker isolate, on the core's jobs.** Model, update,
   runner, render bridge, `QueueClient`, background keep-alive, the engine
   kept on Android; the worker's jobs and `finish` are the core's from the
   first commit (no interim JSON store). Ships: the queue, the update
   tests, kill-and-resume.
6. **The JSON state import.** Ships: an upgrade from the owner's homes.
7. **Delete the old code** (§13), reword the copy (§15), update the docs.

## 18. Review Focus

The inputs most likely to bite a person:

1. **Kill mid-publish.** The app is killed between writing a version's
   content files and committing its step, or between the commit and the
   reply step. Expected: one version, one reply, after resume; never two.
2. **A worker isolate dies and restarts.** The worker throws mid-job and
   the client starts a new one while the old one's last commit may or may
   not have landed. Expected: the new worker owns the queue only once the
   old one's stream has ended, rebuilds its jobs from the core, and
   resumes each from its row; any repeat is a model turn, never a publish
   or reply.
3. **Two claimants of the owner token.** A second worker (a restart racing
   the old one's end, or a second engine) claims the queue. Expected: it is
   refused while the first's isolate lives, gets the claim when that
   isolate's port closes, and the first's later writes are refused; no job
   is run by both.
4. **A WebView reload during a job.** The app's page (or the whole UI
   isolate's engine, on Android after Back) goes away while a render is
   asked. Expected: the job waits for a render, then renders again; it is
   not failed and spends no extra turn.
5. **Two isolates writing.** The UI isolate posts a comment and the worker
   publishes on the same app at once. Expected: the core serializes them;
   a publish on a moved base is a step refused and re-planned, never a lost
   comment or a corrupted draft.
6. **A migration on a large old home.** The owner's phone home at this
   branch's old version 43, possibly large. Expected: a backup first, the
   home adopted by its schema into main's numbering (43 → 45, main's 39 and
   40 added), never an app migration run twice, migration 46 in seconds at
   most, progress on the splash, every artifact and thread kept; a failure
   shows the backup's path.
7. **Origin isolation.** A page tries to read the shell's document or the
   API, to post on `ClaxHost`, or to frame the shell. Expected: nothing
   readable, nothing delivered.
8. **An old daemon still running.** A Mac whose app crashed left its
   `clax` helper running on the same home. Expected: the core opens beside
   it, the helper exits within two stale checks, no write is lost (SQLite's
   locking).
9. **A large call over the channel.** An asset upload of 15 MiB from a page
   through the shell. Expected: chunked, delivered, no frozen UI.
