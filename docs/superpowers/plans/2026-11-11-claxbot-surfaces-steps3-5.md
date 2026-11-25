# ClaxBot surfaces, steps 3 to 5: task breakdown

Read-only planning pass; nothing was built or run. `$W` is `/Users/alex/Devel/empathic/clax-surfaces` (branch `claxbot-surfaces`, HEAD `7ba28ce15`). T7 to T10 have not landed in that worktree, so line numbers in `run.rs`, `script.rs` and `surface.rs` will move; every pointer below is as of that HEAD.

## 0. Where the design and the code disagree

| # | The design says | The code or files say | Consequence |
|---|---|---|---|
| F1 | A binding is a surface, the SHA-256 of its file, a scope and the run's operations | `StoredBinding` v1 holds `granted` (only what is beyond the standing operations) and `hash: null` (`$W/crates/clax-store/src/bot_runs.rs:229-296`); the operations are rebuilt from the provider's code through `SurfaceProvider::standing` (`$W/crates/clax-bot/src/surface.rs:1754`, record 06a96d62) | Bindings version 2 (operations, hash, registration) is the first store task of step 3 |
| F2 | Seven routes ("Steps", and the route table lists seven), plus `GET /api/bot/surfaces` for refusals | Your brief says six | The plan counts eight: seven and the listing |
| F3 | "The sandbox does not change" | The operation list is fixed when the sandbox is built: `$W/crates/clax-server/src/bot.rs:66`, `$W/crates/clax-worker/src/bot.rs:128`. The design's own step-3 note concedes it | Per-run operations is a task (3A-09) |
| F4 | A provider is `read`/`write`/`ending`/`ended` | As built it is `begin` → `StepSession::call` → `finish`, plus `working`, `ending`, `ended`. The seam says a waiting call "has no type here" (`surface.rs:331-337`) | The wire protocol is one adapter. An outside provider has no part in the terminal step (record b2fee1f0) |
| F5 | A parked run is active and keeps its threads | Migration 64's comment: nothing counts `parked` as live. `RunStatus` has no `Parked` (`bot_runs.rs:84-96`), and `row_to_run` refuses such a row (record f1afefe7) | Six places spell the live set out (section 3) |
| F6 | `ask` is `@read @waits`, sent as `"call": "read"` | `shell.d.ts` declares `ask` as `@effect @waits @once` | The fixture follows `shell.d.ts`. With `@once`, "goes out again on the next `serve`" needs `accepted`, so `accepted` is in the smallest cut |
| F7 | `clax bot surface test <file> --against <url>` drives a running host | The host is the HTTP client (it long-polls `serve`), so there is no host URL | Decision O8 |
| F8 | `runs` and `content` are "a namespace of `clax`, provided by the core", always bound | `route` sends any operation whose first segment is not a binding's name to the `clax` binding (`surface.rs:478`); a run with no `clax` gets `unknown_op` | They are two core surfaces with files of their own, read as Clax's own author |
| F9 | `RunPolicy` has `settings`, `history`, `delivery`, `accepts`, `to_model` over `RunInfo` | It has `run_settings`, `limits`, `history`, `is_overflow` over the store's `Run` (`$W/crates/clax-bot/src/policy.rs:528-570`; for-t8 records 6e2998d2, dd0c359d) | `delivery`, `accepts` and `to_model` are step-3 tasks, after T8 |
| F10 | Routes and modules for "surfaces" | `clax_core::surface`, `$W/crates/clax-http/src/surface.rs` and `$W/crates/clax-server/src/routes/surface.rs` already mean "where to ask" | New modules are named `bot_surfaces` |
| F11 | `clax` gains `threads.open`, `ask`, `files.put` | Each adds lines to the declaration every clax.dev model reads; `api.d.ts.golden` would change | They must be granted operations, as `web.*` is (decision O7) |
| F12 | "Two notations": the manifest's canonical bytes name a definition | Built as `canonical_manifest` (record 693b5f0f), still "for the owner to confirm"; nothing stores a hash yet | Decision O1 blocks the first table |

## 1. What each step delivers, and the smallest cut

**Step 3, in the design's terms:** parking and replay, the inbox, `to_model`; registration (configuration and the route), the routes, joins, starting runs; the contract section and fixture; `surface gen`, `test`, `diff`. Done when a test process answers a fixture surface's calls, one a day later by an injected clock, and no Clax source names that surface.

**Step 4:** the content store, handles, upload and read, images to the model on every backend, `content.*`, `clax.files.put`.

**Step 5:** `clax.threads.open`, `clax.runs.wait`, `clax.ask`.

**The boundaries should be re-cut.** Step 3 as written holds about five independent things, and ClaxShell needs under half of it.

| Stage | Holds | Ends with |
|---|---|---|
| **3A. A host answers a run** | Bindings v2 and definitions by hash; runtime registration; start a run; `serve` with `accepted` and `ended`; the outside-provider adapter; per-run sandbox operations; the standard prompt going live (marked `Instruction`); parking and replay | **ClaxShell usable**: `shell.d.ts` registered with the daemon's token, a `shell.triage` run started, `shell.windows` answered at once, `shell.ask` answered a day later, a typed `TriageResult` in `ended` |
| **3B. The rest of the protocol** | `bind` and joins, reader views, effect limits, retries, the inbox, `to_model`, progress events, configuration, leases | `shell.*` in ordinary thread runs on the shell's parts |
| **3C. Tooling** | `gen`, `diff`, `test`, the whole contract section | A host builds from documents alone |
| **4. Content** | As the design | `shell.screenshot` seen by the model |
| **5. Fan-out and questions** | As the design | `shell.triage` fans out and waits |

Two milestones inside 3A: **M1** (register, start, fast calls, typed finish, no parking) and **M2** (the waiting call). M1 is worth an integration check of its own because it exercises every lane but the park.

What is deliberately outside the smallest cut, in the order to do it after: joins and `bind` with reader views (ClaxShell's main daily use), content (`screenshot`, `clip`), fan-out, progress events, the inbox, configuration, tooling.

Three orderings the design's numbering hides:

- **Step 5 needs parking, not the host protocol.** `runs.wait` and `clax.ask` can start once 3A-12 lands, beside 3B and 4.
- **Most of step 4 touches crates 3A does not** (`clax-sample`, a store table, a sniffer), and can run beside 3A. Only its two routes wait for 3A's.
- **`gen` and `diff` are pure functions of a definition** (`clax-surface`, `clax-commands`) and can start today.

**Until step 4 lands:** an operation whose types name `Content` or `Image` is left out of a run's operations, and the registration's answer lists it as withheld with the reason. A request carrying `clip` is refused. `shell.d.ts` registers unchanged (decision D4).

**Check against `/Users/alex/Devel/empathic/claxos/docs/claxbot-services.md`:**

| Asked | Delivered by |
|---|---|
| §1 read, write, effect operations | 3A; the effect limit in 3B-05 |
| §2 request data | 3A: the kind's typed request |
| §3 rules and kinds with allow lists | 3A |
| §4 fan-out with parent links | Step 5 |
| §5 questions, confirming its own effects | 3A (a call that waits); `clax.ask` in step 5 |
| §5 live events | 3B-09 |
| §5 cancel | The existing `POST /api/bot/runs/<id>/cancel`; measurement M8 confirms the bare token passes it |
| §5 confirming `draft.publish` | Out, as the design says |
| §6 registration, `unavailable` | 3A |
| `clax.shell.screenshot()` | Step 4 |

ClaxShell's host already reads the daemon's port and token from `daemon.json` (`/Users/alex/Devel/empathic/claxos/crates/claxos-clax/src/lib.rs:760-1014`), so "with the daemon's token" needs nothing new on its side.

## 2. Task tables

Conventions for every row:

- It ends with the existing suites green and a wasm32 check of `clax-worker`.
- "Must not touch" always includes every golden under `$W/crates/clax-bot/src/*.golden` and `$W/crates/clax-bot/tests/golden/` unless the row names one.
- Tests use scratch homes, free ports and injected clocks, and no sleeps.
- New Rust files need their `BUILD.bazel` entries.

Lanes (disjoint files, so tasks in different lanes run at once): **ST** `clax-store`; **SU** `clax-surface`; **SB** the sandbox crates (`clax-bot-sandbox`, `clax-bot-js`, `clax-worker/src/sandbox`); **CO** `clax-bot`; **HT** `clax-http` and `clax-server`; **CL** `clax-commands`; **TK** `clax-testkit`; **SA** `clax-sample`; **DO** docs; **WB** `web/`.

Layering, checked against `$W/scripts/check-crate-layers.py`: wire types go in `clax_surface::wire`, because `clax-commands` may reach `clax-surface` and not `clax-bot`, and `clax-surface` already has `serde`. No new crate and no `ALLOWED` change.

### Stage 3A

| ID | Lane | What | Touches | Must not touch | Held by | Depends on | Parallel with |
|---|---|---|---|---|---|---|---|
| 3A-00 | CO | **Spike, not merged.** Park by replay through the generic step with `FakeSurfaceProvider`: a call that does not answer, the sandbox left, the step run again from the journal. Find how QuickJS and the Worker's loader leave a script whose host call never resolves without ending as `Cancelled`. Measure replay cost | A scratch branch | The branch | Findings as qualifier records on `script.rs` and `surface.rs` | T10 | Everything but 3A-11, 3A-12 |
| 3A-01 | ST | **One migration for the whole of 3A** (next free number; 65 unless T8 or T10 took it). Tables `bot_surface_defs` (hash, canonical text), `bot_registrations`, `bot_pending_calls`, `bot_run_ends`. `ALTER TABLE bot_runs ADD COLUMN` `started_by`, `parent_run`, `parked_at`, `parked_ms`, `waiting_on`, `pieces_json`. `bot_runs_live` dropped and made again with `parked`. No rebuild | `$W/crates/clax-store/src/migrations.rs` | `bot_runs.rs` behaviour; `clax-bot` | A migration test from a fixture at 64 holding a mid-run; the cost per run in `$W/crates/clax-worker/tests/store_on_vfs.rs` | O1 | 3A-06 to 3A-10 |
| 3A-02 | ST | `RunStatus::Parked`. Each of the six live sets takes it, each with its own test. `park_run` (claim released, `waiting_on`, `due_at`) and `unpark_run`, which adds to `parked_ms` | `bot_runs.rs`, `$W/crates/clax-store/src/credits.rs:1817` | Migrations | A test for each set; a second send of a parked run's thread returns the run | 3A-01 | 3A-03, 3A-04 |
| 3A-03 | ST | Bindings version 2 (`operations`, `hash`, `registration`), read beside version 1. A constructor of `NewRun` for any kind: kind, request, bindings, `started_by`, no artifact | `bot_runs.rs:229-500` | `create_run`'s rows for `clax` runs | A version-1 run made before and resumed after; refusal of any other version | 3A-01, T10 | 3A-02, 3A-04 |
| 3A-04 | ST | New `bot_surfaces.rs`: definitions put and read by hash; registrations upserted, read, listed, touched; audit kind `bot.surface.registered` | New file, `lib.rs` | `bot_runs.rs` | Unit tests; an upsert with the same hash is a no-op | 3A-01 | 3A-02, 3A-03, 3A-05 |
| 3A-05a | ST | Pending calls: `open_call` (needs the claim), `accept_call`, `answer_call` (journals into `bot_calls` in one transaction, authorised by the pending row and not by a claim; makes the run due; a second answer changes nothing; an unknown or expired ID is ignored), `expire_calls(now)`, `calls_for(registration)` | New `bot_pending.rs` | `call_entry`'s claim rule (`bot_runs.rs:1488-1500`) | An answer with no claim held; an answer twice; an answer after expiry; another run's ID | 3A-01 | 3A-04, 3A-05b |
| 3A-05b | ST | Run ends: record with the terminal commit, list undelivered, acknowledge, prune after seven days | `bot_pending.rs` or a new file | The step commit's other rows | A crash between commit and delivery leaves one undelivered end | 3A-01 | 3A-05a |
| 3A-06 | SU | `clax_surface::wire`: serde types for the registration body and answer, `serve` (calls `read`, `write`, `ended`; answers `result`, `error`, `accepted`, `retry_after`), the start body, error codes, size limits. A JSON fixture of one whole exchange, which is the contract fixture | New `$W/crates/clax-surface/src/wire.rs`, `src/tests/wire/` | The definition model | Every fixture message round-trips; unknown fields refused | None | Everything |
| 3A-07 | SU | The check changes decisions O2, O3 and D2 settle: absent and empty `@allow` told apart; invisible and direction-changing characters refused; a required parameter admitting `null` refused in a host's file | `model.rs`, `allow.rs`, `check.rs`, `manifest.rs` | `clax.d.ts`'s printed declaration | The three example files still convert both ways; `api.d.ts.golden` unchanged | O2, O3 | Everything |
| 3A-08 | SU | Two pure functions: a run's operations (the file ∩ operator ∩ starter ∩ kind, with those withheld for `Content`), and the places in a kind's request typed `Instruction` | `model.rs` or a new `run_ops.rs`; `value.rs` | Checks | Table tests over `shell.d.ts`, `review.d.ts`, `fake.d.ts` | 3A-07 | 3A-06, ST, SB |
| 3A-09 | SB | Per-run operations: `Script` carries its run's operation list (default: the sandbox's own). QuickJS and the loader build `clax` from it | `$W/crates/clax-bot-sandbox/src/lib.rs:124-160`, `$W/crates/clax-bot-js/src/lib.rs:158-203,402`, `$W/crates/clax-worker/src/sandbox/mod.rs:189-227,355` | `clax-bot` | A conformance case: two scripts on one sandbox with different operations; `validate_ops` refusals as a host error, not a panic | None | Everything |
| 3A-10 | CO | The run's lifecycle as a pure function: runnable, parked, due, ended; events answer, deadline, cancel, restart, message. No callers | New `$W/crates/clax-bot/src/lifecycle.rs` | `run.rs`, `driver.rs` | A table test, no store and no clock | None | Everything |
| 3A-11 | CO | The seam's third answer: `Answer::waiting`; a `CallBroker` trait (send, await until a time, by the injected clock); `Delivery` and `RunPolicy::delivery` with the daemon's defaults; the fake gains an `@waits` operation and a way to answer later; provider cases for it | `surface.rs`, `policy.rs`, `fake.d.ts`, `provider_conformance.rs` | `script.rs`, `run.rs` | The provider cases on the fake | 3A-00, T8 | 3A-10, ST, SU, SB, HT-18 |
| 3A-12a | CO | The script step parks: a waiting answer not in by `park_after` leaves the sandbox by 3A-00's way, drops the sessions, and the tick answers `Wait::Parked`. The driver's `settle` parks the run and `resume` re-arms its deadline. The step run again is answered from the journal | `script.rs:132-177,310-345`, `run.rs` (`Wait`, 514), `driver.rs:165-210,313-371` | `clax/*`; payload shapes | A test parks, resumes and crashes one step and counts the fake's effects (the design's Anvil rule); "a day later" by the clock | 3A-02, 3A-05a, 3A-10, 3A-11 | HT, TK, DO |
| 3A-12b | CO | The accounting: parked time outside `max_run_minutes`; `max_parks_per_step`; `max_parked_runs` for a person; a deadline's error journaled as `expired`, `unknown_outcome` or `unavailable`; a cancel of a parked run ends it | `run.rs`, `policy.rs`, `driver.rs` | The sandbox | `tests/limits.rs` extended; one test per error | 3A-12a | 3A-13, 3A-14 |
| 3A-13 | CO | The outside-provider adapter: definition by hash from the store; a session with no state; a call sent through the broker with its key and deadline; `ending` answers no fields; `ended` records the run's end for delivery | New `$W/crates/clax-bot/src/outside.rs` | `clax/*` | The provider conformance suite over an in-memory broker | 3A-04, 3A-05, 3A-11 | 3A-12, 3A-15 |
| 3A-14 | CO | The provider registry behind what T8 left: a binding to its provider (`clax`, a registration's adapter, or one answering `unavailable`). The run's operations to the sandbox (3A-09). One declaration over all bindings | `run.rs` (today's `script_step`, 1724-1750), `api.rs` | `Deps::surfaces` users in tests, until they move | `tests/generic.rs` with two bindings; the declaration for `clax` alone byte-equal | 3A-03, 3A-09, 3A-13 | 3A-15 |
| 3A-15 | CO | The standard prompt goes live for a kind that is not Clax's: the request's `Instruction` strings marked as O4 decides; a registration's prompt file; each piece's name and hash recorded | `prompt.rs`, `prompt/standard.rs:16-34`, `prompt/file.rs` | `clax.prompt.md`; the eight system goldens | `prompt.standard.golden` re-pinned; a new golden for `shell.triage` from `shell.d.ts` | 3A-08, O4 | 3A-12 to 3A-14 |
| 3A-16 | CO | `ended` redelivered until acknowledged: on driver start and on a timer of `clax_rt::time`; pending calls expired on the same pass | `driver.rs` | `run.rs`'s terminal commits | A crash hook between commit and `ended`; the fake is told once more, harmlessly | 3A-05b, T7 | 3A-12 |
| 3A-17 | HT | The hub: `CallBroker` over the store's pending calls with a wake per registration; "connected" by an injected clock; `serve` as a plain function (answers in, calls out) | New `$W/crates/clax-http/src/bot_surfaces.rs`; a field on `Bot` (`$W/crates/clax-http/src/bot.rs:67`) | Routes | Unit tests with `ManualSleeper` (`$W/crates/clax-testkit/src/clock.rs`) | 3A-05, 3A-06, 3A-11 | 3A-18, CO |
| 3A-18 | HT | `PUT /api/bot/surfaces/<name>` and `GET /api/bot/surfaces`. One extractor for "the daemon's token and nothing else". Native only, and refused on a hosted daemon | New `$W/crates/clax-server/src/routes/bot_surfaces.rs`; `router.rs` near 590 | `routes/bot.rs`; `need!` declarations | New `tests/api_bot_surfaces.rs`: anonymous, owner browser cookie, viewer, `Clax-Grant`, token, hosted; reserved names; each limit; a bad definition's reason | 3A-04, 3A-06, 3A-07 | 3A-17, 3A-19 |
| 3A-19 | HT | `POST /api/bot/surfaces/<name>/runs`: the request checked against the kind, the operations computed, the refusal, key source and credits rules of `start_run` reused, the driver told. `bind` answers `not_yet` | `routes/bot_surfaces.rs`; a shared piece out of `routes/bot.rs:859-899` | `start_run`'s behaviour for threads | A bad request changes nothing; another surface's kind refused; a person with no credits refused as today | 3A-03, 3A-08, 3A-18 | 3A-20 |
| 3A-20 | HT | `POST /api/bot/surfaces/<name>/serve` over the hub, with body limits and `wait_ms` capped at 12 s | `routes/bot_surfaces.rs`, `router.rs` | The stream routes | An answer for another registration's call ignored; a late answer ignored; a call out again until accepted | 3A-17, 3A-18 | 3A-19 |
| 3A-21 | HT | Wiring: the daemon's driver built with the registry, the broker and the redelivery pass. The Worker builds with none | `$W/crates/clax-server/src/bot.rs:1150-1240`, `$W/crates/clax-worker/src/bot.rs` | `ServerHost` | `tests/core_start.rs`; the wasm32 check | 3A-14, 3A-16, 3A-17 | 3A-22 |
| 3A-22 | TK | `FakeHost`: a client of the protocol with scripted answers, delays, drops and reconnects, which counts each key it applied and replays 3A-06's fixture | New `$W/crates/clax-testkit/src/fake_host.rs` | The server | Its own test against the fixture | 3A-06 | Everything |
| 3A-23 | HT | **End to end.** `TestServer`, `FakeMessages` and `FakeHost` with `shell.d.ts` read from `$W/docs/superpowers/specs/2026-11-10-claxbot-surfaces/`. M1, then M2. A source scan holds that no file under `crates/*/src` names the fixture's surface | New `tests/api_bot_surfaces_e2e.rs` | Sources | The test | 3A-12, 3A-19 to 3A-22, T9 | 3A-24 |
| 3A-24 | DO | `docs/contract.md`, "ClaxBot surfaces": both notations, registration, runs, `serve`, errors, the fixture. `docs/usage.md`. The design corrected for F1 to F12 | `$W/docs/contract.md`, `$W/docs/usage.md`, the design | Code | The doc's examples are the fixture's messages, by a test that reads both | 3A-06 | Everything |

### Stage 3B

| ID | Lane | What | Touches | Must not touch | Held by | Depends on | Parallel with |
|---|---|---|---|---|---|---|---|
| 3B-01 | ST | **One migration for 3B and step 4**: `bot_joins`, `bot_messages`, `bot_content`; `reader_view` on registrations | `migrations.rs` | `bot_runs` | A migration test | Review A | 3B-02, 3B-05 |
| 3B-02 | HT | A run with no `clax` binding: `run_access` gives its person and the admins, it is listed under its person and its surface, and its audit records are filed on the person | `routes/bot.rs:630-656,740-830` | `for_reader` | `api_claxbot_run.rs` extended; a reader gets 404 | 3A | 3B-03, 3B-05 |
| 3B-03 | HT, CO | Reader views: a run shows a reader the least any binding allows; `none` shows kind, state, times and the finish's summary | `routes/bot.rs:1468-1530`, `events.rs` | A `clax`-only run's reader view | The reader test at `api_claxbot_run.rs:734` unchanged; a new one with a host binding | 3B-01, O6 | 3B-02, 3B-05 |
| 3B-04 | HT, CO | `bind` at start: a `clax` scope checked as the person (`reads_artifact`, `may_send_at`). `ClaxProvider` serving a run whose kind is not its own (notices, roster, follow-ups) | `routes/bot_surfaces.rs`, `clax/provider.rs` | Thread and brief runs | A host's run with `clax` posts its stop notice on its threads | 3B-02, 3B-03 | 3B-05, 3B-07 |
| 3B-05a | CO | The effect limit: refused `effect_limit` before the provider sees it; a key counts once | `surface.rs` (`dispatch`), `policy.rs` | `clax/*` | A step replayed does not count twice | 3A | 3B-02 to 3B-04 |
| 3B-05b | CO | Retries as `Delivery` says (attempts, backoff, by the injected clock), `retry_after`, `@once` never sent again once accepted | `outside.rs`, `policy.rs` | The store | One test per row of the design's two tables | 3A | 3B-05a |
| 3B-06 | ST, HT | Joins: stored until replaced; `PUT …/joins`; every place that starts a run binds a joining registration (`routes/bot.rs:878,946,1047,1192`, `routes/artifacts.rs:891`, `routes/threads.rs:1426`) | Those, `bot_surfaces.rs` | A run on an artifact with no join | With no join, every golden stands; with one, a thread run's prompt has `# shell` as its last section | 3B-01, 3B-03, 3B-04, O5 | 3B-07, 3B-08 |
| 3B-07a | ST, HT | The inbox's store and route: `POST …/runs/<id>/messages`, checked against the kind's second parameter; `RunPolicy::accepts` | `bot_pending.rs`, `routes/bot_surfaces.rs`, `policy.rs` | `run.rs` | A kind with one parameter refuses; `clax`'s kinds take none | 3B-01 | 3B-06, 3B-08 |
| 3B-07b | CO | Delivery between steps, in arrival order; a message wakes a parked run and its call stays out | `run.rs`, `history.rs`, `lifecycle.rs` | The transcript of a run with no messages | `tests/golden.rs` unchanged; a new test | 3B-07a | 3B-08 |
| 3B-08 | CO | `to_model` once, at a step's commit, with where each value came from (surface, operation, `Instruction` or not). An `Instruction` shown whole keeps its heading | `script.rs`, `policy.rs`, `surface.rs` | Stored messages of earlier steps | The default lets everything through: goldens stand | 3A | 3B-06, 3B-07 |
| 3B-09 | HT, CO | Progress over `serve`: for a run it started, what the starter may see; for one it joined, its state and its own calls | `bot_surfaces.rs` (hub), `events.rs` | `GET …/events` | A joined run's events show no other surface's call | 3B-06 | 3B-10 |
| 3B-10 | HT | Configuration: `[bot.surfaces.<name>]` read at start as a registration; `clax doctor` and the listing say why one was refused; per-surface run limits through the policy | `$W/crates/clax-core/src/config.rs:1080`, `$W/crates/clax-server/src/core.rs`, `doctor.rs` | Other `[bot]` settings | `tests/core_start.rs` | 3A | 3B-09, 3B-11 |
| 3B-11 | HT | The lease: a registration lasts while connected and a short while after; calls then answer `unavailable`; a replaced definition leaves runs in flight on theirs, by hash | `bot_surfaces.rs` | Joins | By the injected clock | 3A | 3B-10 |
| 3B-12 | WB | The run view: a parked state with what it waits on; the three new outcomes; calls answered from the journal | `$W/web/` | Thread pages | The web unit and e2e tests | 3A | Everything |

### Stage 3C

| ID | Lane | What | Touches | Held by | Depends on | Parallel with |
|---|---|---|---|---|---|---|
| 3C-01 | SU, CL | `surface gen --ts` | New `$W/crates/clax-surface/src/gen_ts.rs`; `$W/crates/clax-commands/src/commands/bot.rs` | A golden for each example file | None | **Can start today** |
| 3C-02 | SU, CL | `surface gen --rust` | New `gen_rust.rs`; `bot.rs` | Goldens; the output compiled in a test target | None | 3C-01 |
| 3C-03 | SU, CL | `surface diff`: whether runs pinned to the old file can be served on the new | New `diff.rs`; `bot.rs` | A table of changes, each safe or not | None | 3C-01, 3C-02 |
| 3C-04 | SU, CL, TK | The host conformance cases as a driver with no I/O in `clax_surface::wire`, and `surface test` as O8 decides | `wire.rs`, `bot.rs` | `FakeHost` passes; a broken `FakeHost` fails each case | 3A, O8 | 3C-05 |
| 3C-05 | DO | The contract section whole (joins, messages, content, events, configuration); a note for ClaxShell on what to delete (`host/fanout.rs`) | `docs/contract.md` | The fixture test | 3B | 3C-04 |

### Step 4

| ID | Lane | What | Touches | Must not touch | Held by | Depends on | Parallel with |
|---|---|---|---|---|---|---|---|
| 4-00 | n/a | **Measure**: the Worker's blob limit; what each backend takes as an image in a tool result; how models are known to take images | Nothing | n/a | Records on the design's "Not yet tried" | None | Everything |
| 4-01 | ST | Content rows and bytes by SHA-256 through `ContentStore` (`$W/crates/clax-core/src/content/mod.rs:257-365`); 20 MiB an item, 100 MiB a run; pruned with the journal | New `bot_content.rs`; `prune_ended_runs` (`bot_runs.rs:1679`) | Assets | Limits; prune | 3B-01 | 4-02, 4-06 |
| 4-02 | CO | Sniffing: media type and dimensions of PNG, JPEG, WebP and GIF from the bytes, starting from `png_size` (`$W/crates/clax-core/src/clips.rs:191`). No `unsafe`, no dependency | New `$W/crates/clax-core/src/media.rs` | `clips.rs` behaviour | Truncated and lying files refused | None | **Can start beside 3A** |
| 4-03 | SU, CO | Handles: a provider's `{id, meta}` filled in from the store before the result's check; `shown: false`; the model's declaration declares the four core types (records 6e77f0be, 07146951, 08306dcb) | `value.rs`, `print.rs`, `surface.rs` | `api.d.ts.golden` | `Image` that is not an image fails `invalid_result` | 4-01, 4-02 | 4-04 |
| 4-04 | CO | The `content` core surface: `text` (paged as `web.fetch`), `get` | New `core/content.d.ts` and provider | `clax.d.ts` | The provider cases | 4-01, 3A-14 | 4-05 |
| 4-05 | HT | `PUT …/content` and `GET …/content/<id>`: the type checked against the bytes; a read only of a handle that was an argument of that run's call to that surface | `routes/bot_surfaces.rs` | The asset routes | The conformance rule: each route with another run's IDs | 4-01, 4-02, 3A-18 | 4-04, 4-06 |
| 4-06a | SA | A tool result's images on the Anthropic client | `$W/crates/clax-sample/src/messages.rs:120-145` | Requests with no image | Byte-equal requests without images | 4-00 | **Can start beside 3A** |
| 4-06b | SA | The OpenAI translator carries them | `messages/openai.rs:87-110` | The same | Its tests | 4-06a | 4-06c |
| 4-06c | SA | The Gemini translator carries them | `messages/gemini.rs:56,109-124` | The same | Its tests | 4-06a | 4-06b |
| 4-07 | CO | Images shown after a script step: four at most, in order; one left out says why; none for a model that takes none; a request's images with the first message; `{core.images}` on | `script.rs:181-225`, `prompt.rs` | A tool result with no handle | The tool-result goldens stand; new ones | 4-03, 4-06a | 4-08 |
| 4-08 | CO | `clax.files.put(path, c)`, as a granted operation | `clax.d.ts`, `clax/draft.rs` | `api.d.ts.golden` | A golden for the granted shape | 4-03, O7 | 4-07 |
| 4-09 | HT | End to end: `shell.screenshot` to the model to the draft; a request with `clip` | `tests/api_bot_surfaces_e2e.rs` | n/a | The test | 4-04 to 4-08 | 4-10 |
| 4-10 | DO | Contract and usage | Docs | n/a | The fixture | 4-05 | 4-09 |

### Step 5

| ID | Lane | What | Touches | Must not touch | Held by | Depends on | Parallel with |
|---|---|---|---|---|---|---|---|
| 5-00 | n/a | **Measure**: a question with no agent session (`create_question_keyed`, `$W/crates/clax-store/src/questions.rs:715`); how a child's spend can count on its parent's run; the Worker's alarm for a parked deadline | Nothing | n/a | Records | None | Everything |
| 5-01 | ST | The parent link's functions: children of a run, their count, depth; a child's end answers its parent's pending call in the terminal commit's transaction | `bot_runs.rs`, `bot_pending.rs` | Migrations (the column is 3A-01's) | A child ending while the parent is not parked | 3A-05a | 5-02, 5-05 |
| 5-02 | CO | The `runs` core surface: `wait`, which parks until each child has ended | New `core/runs.d.ts` and provider | `clax.d.ts` | Park, child ends, resume; a run that is not the caller's refused | 3A-12, 5-01 | 5-03, 5-05 |
| 5-03 | CO, HT | `clax.threads.open`: as the person, on an artifact they may comment on; a child run; `limit` past the settings; a granted operation | `clax.d.ts`, `clax/`, `host.rs`, `$W/crates/clax-server/src/bot.rs` | `api.d.ts.golden` | `host_conformance` gains its cases; a withheld artifact answers as an absent one | 5-01, O7 | 5-02, 5-05 |
| 5-04 | CO, ST | A child's steps and spend on the caller's budget | `run.rs` (`must_stop`), `credits.rs` | A run with no parent | `tests/money.rs` | 5-03, O9 | 5-05 |
| 5-05 | CO, HT | `clax.ask`: a question in Clax's own UI, a call that parks, the answer an `Instruction` | `clax.d.ts`, `clax/`, `routes/questions.rs` | Agent sessions' questions | `api_questions.rs`; park and resume | 3A-12, 5-00, O7 | 5-02 to 5-04 |
| 5-06 | HT, WB | The roster says "Waiting on …" (record 6b08ec29); a thread shows its children | `bot.rs` (`show_working`, 1085), `web/` | A run that never parks | `api_working.rs` | 5-02, 5-05 | 5-07 |
| 5-07 | HT | The Worker: parked deadlines from the alarm; `test-bot.mjs` parks and resumes | `$W/crates/clax-worker/src/bot.rs`, `test-bot.mjs` | n/a | `just worker-test` | 5-02 | 5-06 |
| 5-08 | HT | End to end: `shell.triage` opens two threads, waits, finishes with what each did | `tests/api_bot_surfaces_e2e.rs` | n/a | The test | 5-02 to 5-04 | n/a |

## 3. Measurements needed first

Line numbers in this section are as of commit 7ba28ce15, the HEAD named at the top; the code has moved since.

**Before 3A**

| # | Read or measure | Where |
|---|---|---|
| M1 | What T8 and T10 landed: how the tick holds providers, whether `Run` now takes a NULL artifact, and whether either took migration 65 | `$W/crates/clax-bot/src/run.rs` (today 1724-1750, `Deps::surfaces` 289); `bot_runs.rs:145-183`; the last entry of `migrations.rs` (64 at 2152-2261) |
| M2 | The six places that spell the live set | `bot_runs.rs:631`, `:1065`, `:1156`, `:1383`; `credits.rs:1817`; the index at `migrations.rs:2260` |
| M3 | The journal's claim rule a parked answer must pass another way | `bot_runs.rs:712-720` (`check_claim`), `:1488-1500` |
| M4 | How a script leaves the sandbox without `Cancelled` | `script.rs:310-345,407-411`; `$W/crates/clax-bot-js/src/lib.rs`; `$W/crates/clax-worker/src/sandbox/mod.rs:468-580`. This is 3A-00 |
| M5 | Replay cost: the script's work up to the awaited call, for 1, 10 and 100 calls | 3A-00, on the fake |
| M6 | The driver's waits and how `resume` re-arms `due_at` | `driver.rs:39-42,165-210,313-371` |
| M7 | That a `Clax-Grant` request to a route with no `need!` is refused before the handler, and what tells a token by hand from an owner's agent | `$W/crates/clax-http/src/agent.rs:94-120,747,1292-1345,1363-1380,2191`; `auth.rs:22-48` |
| M8 | That the bare token passes `cancel` and `GET /api/bot/runs/<id>` | `routes/bot.rs:663-674,1211`; `$W/crates/clax-http/src/viewer.rs:47` (`SameOrigin`) |
| M9 | The request deadline `serve` sits under (30 s by default) and which router group it joins | `$W/crates/clax-http/src/state.rs:180,476`; `router.rs:558,795,903` |
| M10 | How native-only routes are kept off the Worker | `router.rs:15,493,865,874,907-916` |
| M11 | The migration's cost on the Worker's VFS (16 MiB a transaction) | The comment at `migrations.rs:2186-2202`; `$W/crates/clax-worker/tests/store_on_vfs.rs` |
| M12 | What must hold before a run is sent the standard prompt | `prompt/standard.rs:16-34`; `standard.prompt.md` |

**Before 3B:** every caller of `start_run` (listed in 3B-06); `for_reader` and `event_for_reader` (`routes/bot.rs:1468-1530`); `PAYLOAD_KEYS` and `merge_fields` for a host's fields (`surface.rs:1286-1465`); `BotConfig` (`config.rs:1080`).

**Before step 4:** the Worker's blob limit (the design says the step starts here); `MAX_ASSET_BYTES` is 20 MiB (`$W/crates/clax-core/src/publish.rs:241`); the tool result is built once at `script.rs:181-225`; `Block::ToolResult.content` is already a JSON value (`messages.rs:130-136`), so the Anthropic side may need no new variant.

**Before step 5:** the questions store is keyed by agent session (`questions.rs:703-960`); the roster call (`$W/crates/clax-server/src/bot.rs:1072-1105`); the Worker's alarm budget (`$W/crates/clax-worker/src/bot.rs:43-60,81-94`); `must_stop` for a child's spend.

## 4. Riskiest moves

| Step | Risk | How to de-risk |
|---|---|---|
| 3A | **Leaving the sandbox mid-call (3A-12a).** Today the only exit is `Cancelled`, which stops the run | 3A-00 first. Extend `crashed_run` (`$W/crates/clax-bot/tests/resume.rs:28`) to a step that parks twice. Hold "replay performs nothing twice" by the fake's applied-key count |
| 3A | **A journal write with no claim (3A-05a).** It is the first writer of `bot_calls` outside a driver | One transaction; authorised by a pending row that names the place; the place's primary key refuses a second row. Race test: an answer arriving while a driver resumes the same step |
| 3A | **Input from outside the process for the first time.** Definitions, prompt files, answers | Every reader takes `Limits` already. Fixed body limits per route. A provider's error message is data. Review A's security pass |
| 3A | **The standard prompt reaches a model for the first time** | O4 before 3A-15; the golden read by the owner; one recorded run against a real model at Review A, in a scratch home |
| 3A | **Migration numbering across lanes** | One migration a stage, one owner (3A-01, 3B-01). No other task touches `migrations.rs` |
| 3A | **The two-minute check suite** | No real waits: `ManualSleeper` for `serve`, the deps' clock for deadlines. The end-to-end test is in-process; a real second process runs only at stage reviews |
| 3B | **Joins change a thread run's prompt (3B-06)** | With no join every golden must stand, by test. Old against new on generated contexts, as step 2 did |
| 3B | **A reader seeing a host's data (3B-03)** | Default `none`. `bind` stays `not_yet` until 3B-02 and 3B-03 are in |
| 4 | **Parsing image bytes from outside (4-02)** | Headers only, bounded reads, a table of truncated and lying files; no decoder |
| 4 | **Three backends (4-06)** | 4-00 measures each; one task a backend; requests with no image byte-equal |
| 5 | **A wait that never ends (5-02)** | The lifecycle table (3A-10) gains the child's end; a deadline always exists by default; a cancelled parent ends |
| 5 | **Something a person on clax.dev can see (F11)** | O7: granted operations, off by default; goldens for both shapes |

For the stage reviews, a real fake-host process: a small Rust binary beside `FakeHost` in `clax-testkit`, started against a real `clax` daemon in a scratch home on a free port, as `$W/crates/clax-server/tests/daemon.rs` starts one. `clax-fake-exe` installs shell scripts and is not the right tool for a long-poll client.

## 5. Review stages

One review per task throughout. Big reviews at four boundaries:

| Review | After | Checks |
|---|---|---|
| **A** | 3A-23 (M2) | Whole-branch `just check`. Mutation testing of `lifecycle.rs`, `outside.rs`, `bot_pending.rs`, the hub and the park path. Old against new for clax.dev: every golden, `script_scenarios`, `ending_scenarios`, `host_conformance`, a run stored at migration 64 resumed. **ClaxShell end to end** against `shell.d.ts` with the fake-host process and a real daemon. **Security review** of the three routes, the registration body, answers, and the standard prompt. `just worker-test` |
| **B** | 3B and 3C | The same, plus: the auth matrix for all eight routes; joins and reader views by a second person; the contract fixture replayed by both sides; `surface test` against the fake host |
| **C** | Step 4 | The same, plus: uploads (type against bytes, sizes, another run's IDs); images on all three backends; the Worker's blob limit |
| **D, final** | Step 5 | Whole branch: `just ci`; mutation testing over everything new; old against new with every grant off; ClaxShell's full triage; security review of everything that takes input from outside the process; every open `for-t*` and `status:needs-decision` record closed or re-filed |

## 6. Decisions

**The owner must make these before the task named can start.**

| # | Decision | Blocks | Recommended |
|---|---|---|---|
| O1 | The bytes that name a definition (records f59ab529, 693b5f0f) | 3A-01 | Confirm `canonical_manifest`: no white space, field order part of the hash, said in the contract |
| O2 | What a kind with no `@allow` has (record be0f52aa) | 3A-07, 3A-08 | Absent and empty differ. A host's kind must say `@allow`; Clax's own may leave it out. Fail closed |
| O3 | Invisible and direction-changing characters in a file's words (record acd8413c) | 3A-07, 3A-18 | Refuse them. A registration's rules are reviewed by a person and read by a model |
| O4 | How the person's words are marked in a request, and the owner's reading of the standard prompt's `data` and `images` paragraphs (records ddbcd266, a77dfc87) | 3A-15 | The JSON as it is, preceded by a list of the JSON pointers that are the person's words: nothing inside the JSON can add to it |
| O5 | Standing joins with no approval per artifact (design call 4), given that every owner's agent holds the token and a join puts rules into runs acting as the owner | 3B-06 | Accept, with each registration and join audited, listed in `clax doctor` and on the admin page |
| O6 | Reader views, nothing by default (call 13) | 3B-03 | Accept |
| O7 | Whether `threads.open`, `ask` and `files.put` reach clax.dev runs when they land (F11; call 12) | 4-08, 5-03, 5-05 | Granted operations behind an instance setting, off by default on hosted, so the declaration is byte-equal until evidence says otherwise |
| O8 | What `surface test --against <url>` means (F7) | 3C-04 | The command starts a stand-in daemon on a free port and prints its address and token; the host connects to it |
| O9 | A child's spend on the caller's budget as the meaning of `threads.open` (call 11) | 5-04 | Accept |
| O10 | The design's calls still "awaiting the owner's verdict", each gating its tasks: 1 (two notations: 3A-18), 5 (inbox: 3B-07), 6 (parking by replay: 3A-12), 8 (one door: 3B-08), 9 and 10 (deadlines, retries, `accepted`: 3A-11, 3B-05), 14 (progress: 3B-09), 16 (images on every backend: 4-06) | As listed | Accept each as written; 6 after 3A-00 reports the replay cost |

**These can be defaulted; the owner may overrule.**

| # | Decision | Default |
|---|---|---|
| D1 | Module names, given F10 | `bot_surfaces`, `bot_pending`, `clax_surface::wire` |
| D2 | A required parameter admitting `null` (record 308e480c) | Refused in a host's file |
| D3 | `script.max_calls` under `NOTICE_SEQ` (records 98d6baae, 2a9c97f6) | As the record recommends: cap, clamp, keep the backstop |
| D4 | `Content` operations before step 4 | Withheld, and said in the registration's answer |
| D5 | A dense file with no manifest within the limit (record 4bdca5b4) | Keep the limit; the refusal says so |
| D6 | The outside provider and the terminal step (record b2fee1f0) | No part in it: `ended` only |
| D7 | `Answer::unwritten` kept (records 3c2d69c9, cae5a0a9) | Kept; the adapter never uses it |
| D8 | `thread_ids_json` NOT NULL (record 585c9744) | Already merged as `[]`; close the record |
| D9 | Every figure (30 s, 24 h, 10 s, 8 parks, 20 parked runs, 5 effects, 12 s, 15 s, 7 days) | The design's, as the daemon's default policy |
| D10 | Step 5 on the Worker | The daemon first; the Worker in 5-07 |
| D11 | The manifest's shape and the commands as built (record bb66b1ce) | Confirm; the design's text already follows |

## 7. Explicitly out

Per the design's "Not in this design" and "Registration and authority". None of this is to be built, and the routes refuse it:

- A page registering a surface and running ClaxBot for its viewers; the artifact's declaration and approval for it.
- Hosted providers: a browser tab answering a hosted Clax, a product's account, many people's tabs. The Worker takes no registrations; the routes are native only.
- The services side: tenants, resale, usage and history per end user, isolation guarantees, reseller accounts.
- A Clax artifact nested in another product's review.
- Two bindings of one surface in a run.
- A host confirming Clax's own `draft.publish`.
- A compacting history policy, sessions, working memory (`memory.*`).
- Rebuilding a run's exact context; outcome cases and evals.
- A thread's clip as `clip?: Image` in the `thread` request.
- Speaking MCP's wire protocol; a bridge from an MCP server's tools (record fc50c6c3).
- Constraints JSON Schema can say and TypeScript cannot (`minimum`, `pattern`).
- Timers, heartbeats, exactly-once effects, compensation.
- A general-purpose agent.

### Critical Files for Implementation
- /Users/alex/Devel/empathic/clax-surfaces/crates/clax-bot/src/surface.rs
- /Users/alex/Devel/empathic/clax-surfaces/crates/clax-bot/src/script.rs
- /Users/alex/Devel/empathic/clax-surfaces/crates/clax-store/src/bot_runs.rs
- /Users/alex/Devel/empathic/clax-surfaces/crates/clax-server/src/routes/bot.rs
- /Users/alex/Devel/empathic/clax-surfaces/docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md
