# ClaxBot surfaces, step 2: a core without nouns

The plan for step 2 of
`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`, from a
reading of the run loop as it stands. Nothing here was built or run.
Paths are under `crates/`.

## Measurements

Line numbers in this section are those of `run.rs`, `script.rs`, `history.rs` and `clax/tally.rs` at commit 70e2f2b81, and of every other file at 59065e76b, where this plan was written; the code has moved since.

### Where the nouns are read

**Belongs to the `clax` provider**

| What | Where |
|---|---|
| Thread and brief rules before a call | `clax-bot/src/script.rs:196-222` (`thread_ids` :200, `request` :208, `created` :214) |
| Draft operations, rebase, publish | `script.rs:224-338` (`artifact_id` :241, default `addresses` = `thread_ids` :312); `draft.rs:61,100` |
| Draft load, payload fields, draft commit | `script.rs:552-553` (`base_version`), `:693-697`, `:714` |
| Reply and create tracking, call target | `script.rs:442-443`, `:367` |
| Web grant and source | `script/web.rs:515,521`; `run.rs:278` (`Deps.web`) |
| Prompt and request context | `run.rs:1979`, `:1999-2000` (`request`, `web`) |
| Notices and their wording | `run.rs:1159-1175`, `:1391-1405`, `:1464-1485`, `:1490-1510` (`thread_ids` :1496) |
| Finish's posting rule and web sources | `run.rs:1747-1769` |
| Roster | `run.rs:1569,1618,1660,1781,1802` |
| Draft and thread tallies (`clax::Tally`, which T3 split from `History`) | `clax/tally.rs:21-38`, `:46-51`, `:57-73` |
| Driver start housekeeping | `driver.rs:219-235` (artifact sweep, `start_waiting`) |
| Store | `clax-store/src/bot_runs.rs:604-611` (artifact must exist), `:989-1008` (draft rows in the step commit) |
| Server | all of `clax-server/src/bot.rs`; `routes/bot.rs:859-887`, `:923-1066` |

**Core.** `person`, `model`, `key_source`, `credits_milli`, status, claim,
`due_at`, `created_at`; `must_stop` (`run.rs:1420-1461`); the turn
(`:1962-2403`) apart from `:1978-2001`; `History`'s `messages`, `next`,
`nudged`, `system`, `committed`, `scripts`, `last_usage`; journal lookup
and divergence (`script.rs:340-356`).

**Ambiguous**

| What | Why |
|---|---|
| Audit subject, `rec.ids.artifact` (`bot_runs.rs:639,1029,1043,1081`) | The core writes the records; a run without `clax` must file them on the person |
| `payload.notice`, `posted`, `sources` (`run.rs:1543,1763-1769`) | The core's step payload carries the provider's words, and the run view reads them (`events.rs:323-326`) |
| `Event::Call.target` and `.web`, `step_summary`'s `changed` and `published` (`events.rs:84-93,319-321`) | Provider facts in a core event type |
| `Run::web` | Both the allow-list (core intersection) and a prompt input |
| `run_access` (`routes/bot.rs:630-656`) | A reader is defined through the artifact |

### Endings, roster, follow-ups, exclusivity

| Behaviour | What it assumes | Lives in |
|---|---|---|
| `finish` | Posts the summary on every thread when no thread was replied on since the last draft write or publish. The test is run-wide, not per thread. Sources go at `NOTICE_SEQ+1` otherwise | Run loop, `run.rs:1745-1783` |
| `give_up`, stop, fail | Post on every thread before the commit, journaled under the open terminal step at `NOTICE_SEQ+i`. Cancel and `request_gone` post nothing | Run loop, `run.rs:1530-1620`, `:1785-1804`, `:1150`, `:1986` |
| Roster | `working(true)` on each non-terminal tick; `working(false)` after each terminal commit | Run loop calls; server `bot.rs:1085-1105` |
| Follow-ups | Started from inside `working(false)`, and drained on driver start | Server `bot.rs:1041-1067`, `routes/bot.rs:983-1066`; store `bot_runs.rs:490-587` |
| Exclusivity | A JSON match on `thread_ids_json` among `queued`, `running`, `stopping` | Store `bot_runs.rs:334-345`, `:468`; `routes/bot.rs:870` |

### History, tool result, limits

- **History:** one fold, `History::of`, read every committed step on
  every tick and yielded the messages and the control state. T3 split
  it in three: `Control::of` (`history.rs:128-156`: `next`, `nudged`),
  folded on every tick; `transcript` (`history.rs:221-245`: the
  messages), folded by a tick that takes a turn; and `clax::Tally::of`
  (`clax/tally.rs:42-76`: `replied`, `base`).
- **Form:** step 1's `system` and `user` (`:2360-2363`), each turn's
  assistant blocks verbatim, `next.user`, and each script's stored
  `payload.result`.
- **Tool result:** built once at commit and stored (`script.rs:488-529`,
  `:684-691`), never at history time.
- **`context_window`:** decided in `next_after` (`run.rs:645-647`) and
  committed in the turn's payload as a `give_up`. A provider's
  prompt-too-long HTTP error takes another path, to a `model_error` stop
  (`:2278-2294`).

| Limit | Where | Kind |
|---|---|---|
| `max_steps`, `max_run_credits`, `max_run_minutes` | `run.rs:95-98`, read at `:1440-1455` | `RunSettings`, not optional. Since T4 the run loop reads them from `RunLimits` (`policy.rs`), where each is optional |
| Run time | `run.rs:1197-1213` | Counted from `created_at`; also bounds the model call (`:1217-1224`, `:2091`) |
| `MAX_TOOL_RESULT_BYTES` (100 KiB) | Was a constant of `script.rs`. Since T4 `RunLimits::tool_result_bytes` (`policy.rs`), read at `script.rs:684` | Was a constant; a policy value |
| "at most 64 KiB" | `run.rs:591-596` | Text in the tool description: a byte the model reads. It is the sandbox's limit on a returned value (`Limits::max_result_bytes`), not the tool result's size; since T4 the text is made from `RunLimits::script` |
| `OBSERVE_EVERY` | Was a constant of `run.rs`. Since T4 `RunLimits::observe_every` (`policy.rs`), read at `run.rs:2079` | Was a constant; a policy value |
| `retry_later` 30 s | `run.rs:315` | `Deps` default |
| `STALL_*`, `NOT_OURS_WAIT` | `driver.rs:39-42` | Constants |
| `BOT_JOURNAL_KEEP`, `BOT_DRAFT_KEEP` | `bot_runs.rs:1406,1411` | Constants |
| 50 threads | `routes/bot.rs:1134` | Literal |

### Begin, abandon, replay, and what "park" adds

- **Begin:** `begin_step_on` (`bot_runs.rs:897-952`) inserts the step, or
  bumps `attempt` on the open one. It needs the claim and a `running`
  status.
- **Abandon:** the tick returns `Wait::*` without committing
  (`run.rs:485-497`). `settle` (`driver.rs:313-371`) turns that into a
  delay, records `due_at` (`bot_runs.rs:823`) and releases the claim.
- **Replay:** the journal is loaded per step (`script.rs:554-559`). Host
  calls are answered from it (`:402`), draft operations are executed
  again (`:387-392`), and a replayed publish re-applies its base
  (`:286-294`).

Park adds:

1. **A status.** `status` has a CHECK
   (`clax-store/src/migrations.rs:1559`), so this is a table rebuild. The
   live set is spelled out at `bot_runs.rs:337,755,846,1073`,
   `credits.rs:1817`, the `bot_runs_live` index and the match at
   `bot_runs.rs:915-921`.
2. **A journal write without a claim.** `call_entry` refuses unless the
   driver holds the run (`bot_runs.rs:1188-1189`), and a parked run holds
   none. It needs a pending-call row that authorises one entry.
3. **Due.** `set_run_due(None)` plus `Driver::wake`; `live_runs` must
   include parked runs so `resume` re-arms deadlines.
4. **A way out of the sandbox** that is not `Cancelled`, which
   `script.rs:663-682` treats as stopping the run.
5. **A parked-time accumulator** for the cap.

### Worker and SQL constraints

- **Traits:** `?Send` async traits and cfg'd aliases
  (`host.rs:144-157,261-262`; `run.rs:248-252`).
- **Alarm:** one alarm is at most 15 minutes of wall time, with a budget
  5 minutes under it, and no timer lives between alarms
  (`clax-worker/src/bot.rs:43-60,81-94`). `ended` delivery and park
  timers must run from a tick or an alarm, on `clax_rt::time`.
- **Sandbox:** its operation list is fixed at construction
  (`clax-server/src/bot.rs:66`, `clax-worker/src/bot.rs:128`).
- **Host:** the Worker uses `ServerHost`, so the `clax` provider is
  written once.
- **SQL:** there are no foreign keys. `artifact_id`, `base_version` and
  `thread_ids_json` are NOT NULL, and `request` has
  `CHECK (request IN ('thread','brief'))` (`migrations.rs:1554-1574`).
  There is an index `bot_runs_by_artifact`, an artifact-exists check on
  create, and `audit_query.rs:158` reads `bot_runs`.

### Tests

**Holding behaviour:** `check_host` (`host_conformance.rs:249`), run on
the fake (`clax-bot/tests/rules.rs:94`) and on the server host
(`clax-server/tests/api_claxbot_run.rs:250`); `clax-bot/tests/*` (11
files, 4,031 lines: crash and resume, replay divergence, caps, cancel,
the driver); server tests for follow-ups (`api_claxbot_run.rs:569,890`)
and the reader view (`:734`).

**Not covered:** the system prompt is asserted only by `contains`
(`tests/runs.rs:89`; `prompt.rs:157-266`); tools are asserted by name
only (`runs.rs:94-100`), their descriptions and schemas unpinned; there
is no golden for `request_message`. Not checked, and no pin found for:
step payload shapes, notice wording and journal places, the audit
`ids.artifact`, or a run stored by old code resumed by new code.

## Tasks

Every task ends with the existing suites green and a wasm32 check of
`clax-worker`.

| # | Goal | Touches | Must not touch | Proof | Parallel with |
|---|---|---|---|---|---|
| T0 | Pin the bytes: system prompt (2 kinds × web shapes), `request_message` ×2, the whole first and second request bodies (tools, tool result), each step kind's payload, notices with their journal places, a step's audit record | New `clax-bot/tests/golden.rs` and fixtures | `src/` | The goldens | T1, T2 |
| T1 | One `bot_runs` rebuild: `artifact_id` and `base_version` nullable, `request` unchecked, new `kind`, `bindings_json`, `request_json`, status CHECK admitting `parked` (unused). `NewRun` dual-writes | `clax-store/src/migrations.rs`, `bot_runs.rs` | `clax-bot`, the server | A migration test from a fixture database holding a mid-run | T0, T2 |
| T2 | The seam types, with no callers: `SurfaceProvider`, `StepSession`, `RunInfo`, `Binding`, `End`, `FakeSurfaceProvider`, a provider conformance suite | New `clax-bot/src/surface.rs`, `provider_conformance.rs`; `lib.rs` | `run.rs`, `script.rs` | Conformance on the fake | T0, T1 |
| T3 | Split `History` into the core fold, a message projection and a `clax` tally | `run.rs`, the reads in `script.rs` | Payload shapes | T0; `run.rs:2419` | T6 |
| T4 | `RunPolicy` and a default policy. A blanket impl over `SettingsSource` keeps callers unchanged. Caps become optional, the tool-result limit a setting, and an `Overflow` hook defaults to giving up | New `policy.rs`; `run.rs`, `script.rs` (the tool result's size) | The server, the Worker | `tests/limits.rs`; T0 | T6 |
| T5 | Script calls through a session: generic journal, divergence, checks and dispatch in `script.rs`; rules, draft, publish, reply tracking and web move to `clax/` | `script.rs`, `script/web.rs` (moved to `clax/web.rs` by T5, 0e3dc320f), new `clax/*` | `BotHost`, the server | Resume and replay tests, extended (risk 1) | T6 |
| T6 | A prompt file: `clax.prompt.md` and a renderer behind today's `prompt::system_prompt` and `request_message` signatures | `prompt.rs`, new files | `run.rs` | T0 prompt goldens; old against new on generated contexts | T3 to T5 |
| T7 | Endings: notices, the finish rule, sources, the roster and follow-ups move to the provider's `ending` and `ended` | `run.rs`, `clax/*`, `host.rs`, `clax-server/src/bot.rs:1041-1067` | `routes/bot.rs` | T0 notice goldens; `api_claxbot_run.rs:890` | None |
| T8 | The tick holds `RunInfo` and its providers. The core reads `kind` and bindings; `clax` reads its own columns. The audit subject comes from the binding; driver housekeeping goes behind the provider | `run.rs`, `driver.rs`, `events.rs`, `routes/bot.rs:859-887` | The HTTP run JSON | All suites | None |
| T9 | Typed `finish`: the tool schema comes from the kind's result, with `{summary: string}` for `clax`, and an `invalid_result` retry | `run.rs:580-718`, `:1745` | The prompt | T0 tools golden | None |
| T10 | A run with one fake surface and no `clax`. Delete `api::Performer`, `Deps.host`, `Deps.web` and `BotHost::working` | `run.rs`, `script.rs`, `api.rs`, the tests' `common` | The store | The new test; everything else | None |

Done: T0 (the pins, under `clax-bot/tests/golden/`), T1 (migration 64),
T2 (the seam, `clax-bot/src/surface.rs`, its fake, its conformance suite
and its proof in `tests/seam.rs`), T4 (the run policy,
`clax-bot/src/policy.rs`) and T6 (the prompt file,
`clax-bot/src/clax.prompt.md` and `prompt/`). T4 as built differs from
its row: there is no blanket impl (`Deps::new` sets `DefaultPolicy`, so
callers are unchanged, and a settings type may be a policy of its own),
and the `Overflow` hook is `RunPolicy::history` asked again, whose
answer is checked before it is sent. Also in: the manifest
notation and `clax bot surface check` and `convert`
(`crates/clax-surface`, `clax-commands`). T3 (`history::Control`,
`history::transcript` and `clax::Tally`, in place of one fold). T5:
a script's calls go through the seam. `clax-bot/src/script.rs` is the
generic step (the sandbox, a call's place, the journal, the divergence
check, `dispatch`, what is told of a call, the commit of what the
sessions answered) and names nothing of Clax's; Clax's own surface is
`clax::ClaxProvider` (`clax-bot/src/clax/`: `provider`, `session`,
`rules`, `draft`, `publish`, `replies`, `web`). The step it replaced
is kept for the library's tests as `src/script_reference.rs`, and
`src/script_differential.rs` (both deleted by T10, ebe3d108f, for
`src/script_scenarios.rs`) drives thirty runs both ways, the crashed
ones also resumed across the two, and compares everything each leaves;
`tests/generic.rs` runs the step with the fake surface alone. As built
it differs from its row: the web performer was not split off; a test
gives a run other surfaces through `Deps::surfaces`, until T8 reads a
run's bindings; and one journal row changed as decided (a web
operation refused because the run lacks it, or the host has no source,
is `{code, message}`: `tests/golden/journal.web-disabled.json`). What
T7, T8 and T10 must do that T5 could not is in the qualifier records
tagged `for-t7`, `for-t8` and `for-t10` on `clax-bot/src/script.rs`,
`run.rs`, `surface.rs` and `clax/`. T7: a run's end goes through the
seam. `clax-bot/src/ending.rs` is the run loop's part of the terminal
step (`Tick::end`: the step's own fields, each binding's `ending`
asked and joined, a failure counted, the commit, then `ended`) and
`Tick::working`; the notices and their words, the finish rule, the
sources, the roster and the follow-ups are `clax`'s provider's
(`clax-bot/src/clax/ending.rs`), which calls the `BotHost` methods as
they were, so the server and the Worker did not change. The endings it
replaced are kept for the library's tests as
`src/ending_reference.rs`, and `src/ending_differential.rs` (both
deleted by T10, 1e9aefb5b, for `src/ending_scenarios.rs`) ends
thirteen runs both ways, plainly and with the driver dying at three
points of the terminal step, resumed each way and across the two;
`tests/generic.rs` ends a run of the fake surface alone each way.
As built it differs from its row: `host.rs` and
`clax-server/src/bot.rs` are untouched; one behaviour changed as the
seam's contract decided (a `finish` whose sources cannot be read is
tried `max_commit_failures` times and the run then ends `failed`,
where it was tried without end); a starting driver tells each
provider again of the ended runs it still awaits
(`SurfaceProvider::awaiting_ended`, `Driver::start`), beside
`BotHost::start_waiting`, which T8 moves; and a host's surface may not
be named as a field of a run's step (`clax_surface::STEP_FIELDS`).
`BotHost::working`, `BotHost::start_waiting` and `Deps.host` are still
read (by `clax`'s provider, the driver, the turn's request context and
the references) and are T8's and T10's. T8: a tick reads its run's
kind, request and bindings as stored (`RunAsMade`,
`Store::run_with_steps_as_made`) and holds the provider of each binding
from a registry (`clax-bot/src/registry.rs`, `Deps.providers`, with a
provider that answers `unavailable` for a surface that has none), the
policy is told a `RunInfo`, the first turn's request, the audit subject
and a starting driver's housekeeping are behind the provider, and
`tests/generic.rs` runs runs of the fake's kind with no `clax` provider
registered; `Deps.host`, `Deps.web` and `api::Performer` stay for T10.
T10: the references and their comparisons are gone (the old fold, the
old prompt code, the script step and the endings before the seam).
What the comparisons held stands on the product alone: the folds on
their own tests and the generated histories of
`clax-bot/src/history_fixtures.rs`; the request message on its goldens'
words and the request's JSON over every text in every place
(`src/prompt/request_words.rs`); the thirty script runs and the thirteen
endings, plainly and cut at each point of the terminal step, on
fixtures written while the two ways agreed (`src/script_scenarios.rs`,
`src/ending_scenarios.rs`, `clax-bot/tests/scenarios/`). `Deps` holds no
host and no web source: `Deps::new` keeps its signature and hands the
host to the registry, which makes `clax`'s provider over it, and
`with_web`/`without_web` give that provider another source, so the
server and the Worker did not change. `api::Performer` is gone: which
part of `clax`'s provider performs an operation is its session's own
table. As built it differs from its row: there is no new test of a run
with one fake surface and no `clax`, since `tests/generic.rs` (T5, T7,
T8) is that test; `BotHost::working` stays, as Clax's host interface,
called by `clax`'s provider alone; and a starting driver still sweeps
the artifacts that have no version before it tells the providers
(`Driver::start`), for the reasons the qualifier record on
`clax-bot/src/driver.rs` gives.

What T1's reviews add to later rows:

- T8 and T10 say "must not touch the store", but today's `Run` cannot
  hold a run with no artifact (`artifact_id: String`, `RequestKind`).
  The task that first writes a NULL artifact, another request kind or
  `parked` widens `Run`, `RunStatus` and `RequestKind` in the same
  change.
- Migration 64 already admits those rows, and `row_to_run` refuses to
  read one it does not know. So that task also carries a migration (a
  marker is enough), so that an earlier build refuses the database
  instead of failing on one row: an unreadable row fails a whole run
  listing, the comment that follows it, and the driver's resume.
- `parked` is in no live or active set yet. The places that spell those
  sets out are listed in the store's qualifier records on
  `bot_runs.rs`.

Order: T0, T1 and T2 together; then T3, T4, T5 in series with T6 beside
them; then T7, T8, T9, T10. T5 is the largest; split off the web
performer if a reviewer balks.

## Riskiest moves

1. **T5, draft, publish and replay behind a session.** Before starting,
   extend `crashed_run` (`tests/resume.rs:28`) to a script that writes,
   rebases, publishes, replies and fetches. Assert that the journal rows,
   the fake host's writes and the payload are identical across the crash.
   Spike the session trait on a replayed publish first.
2. **Byte identity across T6 and T9.** Land T0 first. Keep the old prompt
   functions under `cfg(test)` and compare old against new over generated
   contexts until T10.
3. **T1 and T8, the rebuild with runs in flight, on both targets.** Use a
   fixture database from today's schema with an open script step and its
   journal: migrate, resume, finish. Dual-write, drop no legacy column in
   step 2, and run the Worker's `test-bot.mjs` before merging.
