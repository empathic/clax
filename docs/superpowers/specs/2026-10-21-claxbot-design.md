# ClaxBot: Clax's own agent, credits, admins and audit

Status: approved for implementation by the owner (2026-10-21). Mockups:
the "ClaxBot Mockups" artifact, versions 1–9, on the owner's local Clax.
This document is the source of truth; where it and the mockups differ, it
wins.

## 1. Intent

A person on clax.dev or in the Clax phone app often wants a comment
resolved, or a new artifact built, with no local agent running. ClaxBot is
an agent that lives inside Clax itself: on the hosted Worker (clax.dev) and
in the localhost daemon. It is a functional stand-in for a Claude Code
session over Clax, reachable only through Clax, with every step visible
and recorded in Toolpath.

It thinks in **code mode**: instead of one tool call per action, it writes
a JavaScript program against Clax's API, which Clax runs in a sandbox whose
only way out is that API. Every API call goes through Clax's own code, as
the person the run acts for, under their rights.

Running it costs **credits**: one unit of spend, attributable to every
underlying cost, priced at cost plus a margin. A free tier resets weekly;
admins grant lifts. No payments in v0.

## 2. Owner decisions (verbatim intent)

- D1. Runs on clax.dev infrastructure **and** in the localhost daemon.
- D2. Per-user feature flag, set by admins from an admin dashboard.
- D3. Admin dashboard also configures the provider: API key, endpoint,
  models. Anthropic Messages API for now.
- D4. Local configuration from a file loaded with **figment**; **all**
  existing config (`[serve]`, `[sample]`, `[hosted]`, `[toolpath]`,
  `[questions]`, `bin`) moves to figment too. A settings screen on the
  localhost server's web UI.
- D5. Sandboxed, **no real shell** and **no open internet** in v0: scripts
  reach only Clax's API.
- D6. Code mode: the agent generates scripts that Clax runs and resolves
  (intercedes) against its own environment; on Cloudflare where possible;
  logged in Toolpath; visible to the person.
- D7. Name: **ClaxBot**.
- D8. A **New artifact tile** in the gallery: name, description, and a
  toggle "ClaxBot builds it now".
- D9. Usage-based pricing with a free tier. v0: no Stripe, but usage
  monitoring, a per-user limit that is displayed, the UX of going over,
  and enforcement, plus a separate per-user **unenforced** flag (meters,
  never blocks).
- D10. A **credit** is the unit of spend. It is spent on script CPU×RAM
  time, model tokens, and any other underlying cost. Every credit must be
  attributable to an underlying cost, with meaningful margin.
- D11. People **may bring their own model key**; the harness still costs
  credits (compute and the rest) even when they pay for tokens. A hefty
  free tier.
- D12. An **admin** class. The instance owner is always an admin. **Only
  the owner grants or removes admin.** Admins can change the instance key.
- D13. **All views and ops are recorded in Toolpath.** A **full audit UI**
  consumes the Toolpath records.
- D14. Admins grant **lifts** in free credits. Free credits **reset
  weekly** (like Claude plans).
- D15. Model fields are selectors that enumerate the provider's models once
  the key and URL are set (with a custom ID escape hatch).
- D16. Built off `main`, in this repository (localhost daemon and the
  clax.dev Worker are both here). The `flutter-app` branch's Dart harness
  (`apps/clax_app/lib/agent/`) is prior art, not a dependency.

## 3. Non-goals for v0

Payments; open internet from scripts; a shell or real file system; headless
rendering or screenshots (see §6.6); bringing a non-Anthropic provider;
replacing the Flutter app's own harness.

## 4. Architecture

### 4.1 Crates (update `scripts/check-crate-layers.py` `ALLOWED`)

| Crate | Depends on | Holds |
|---|---|---|
| `clax-core` | — | config types (figment), shared vocabulary |
| `clax-store` | CORE | migration 48, roles, flags, credits, cost lines, usage ledger, sealed secrets, bot runs/steps, new audit kinds, audit queries |
| `clax-sample` | CORE | + `messages`: Messages API tool-use streaming client and `list_models`, both targets |
| `clax-bot-sandbox` (new) | — | the sandbox contract, both targets: `Sandbox`, `Host`, `Script`, `Limits`, `ScriptOutcome`, the shared JavaScript prelude, and the conformance suite |
| `clax-bot` (new) | CORE, `clax-sample`, `clax-bot-sandbox` | the run state machine, prompts, the script API definition and its `.d.ts`, metering arithmetic; traits `BotHost` (Clax operations), `RunStore` |
| `clax-bot-js` (new, native only) | `clax-bot-sandbox` | `Sandbox` over embedded QuickJS (`rquickjs`): CPU budget via interrupt handler, memory limit, host-call bridge |
| `clax-http` | + `clax-bot` | admin/settings/credits/bot/audit routes, `BotHost` implementation over the store and existing write paths |
| `clax-server` | + `clax-bot-js` | native driver (tokio), wires the QuickJS sandbox |
| `clax-worker` | (own workspace) | Worker Loader sandbox, DO-alarm driver, `CLAX_KEK` |

No `unsafe` in workspace crates (dependencies may use it). No crate turns
on another's features for tests.

### 4.2 Run model (durable, resumable)

A **run** is ClaxBot working one request: a thread sent to it, or a new
artifact's brief. It is a state machine of **steps**, each committed
before the next starts, so a run survives a daemon restart or a Durable
Object eviction:

- Step kinds: `turn` (one Messages API request and its streamed reply),
  `script` (one sandboxed program), `finish`, `give_up`, `stopped`.
- A driver calls `tick(run)`: load state, check credits (§7.4), execute
  exactly one step, write the step, its usage rows and its Toolpath
  events in one transaction, schedule the next tick.
  - Native: a tokio task per live run, bounded concurrency (default 4).
  - Worker: the DO alarm (already the daemon's loop driver,
    `crates/clax-worker/src/instance.rs`), set to now while runs are
    runnable; each invocation ticks as many steps of as many runs as fit
    in its wall-time budget (`crates/clax-worker/src/bot.rs`
    `alarm_budget`: Cloudflare's 15-minute alarm limit less a margin,
    never less than `min_turn_budget`), then is set for the next wake.
- A step that was started but not committed is re-executed on resume
  (idempotent: model turns are re-requested; script API writes are
  committed only at the step's commit — scripts write a draft, §6.3).
- Caps (config, §8): max steps per run (default 40), max credits per run
  (default 200), max wall time per run (default 30 min).
- Cancel: `POST …/cancel` marks the run `stopping`; the driver stops at
  the next boundary with a `stopped` step.

### 4.3 Model loop

Messages API, streaming, `tool_choice: auto`. Tools given to the model:

- `run_script {title, code}` — the code-mode tool. `code` is the body of
  an `async` function whose `return` value (JSON-serialised, capped at
  64 KiB) is the tool result, along with captured `console.log` lines and
  the list of calls made.
- `finish {summary}` — ends the run successfully; the summary is posted
  as the reply when the run came from a thread and no reply was made.
- `give_up {reason}`.

The system prompt (cached) carries: what ClaxBot is, the request, the
artifact's metadata, the Clax page contract (`docs/contract.md` "Page
contract"), and the generated TypeScript declaration of the `clax` API
(§6.2). Default model `claude-sonnet-5-5`; adaptive thinking; history is
append-only; `max_tokens` per turn from config. A turn with no tool call
is nudged once ("Call run_script, finish or give_up"), then ends the run
as `give_up`.

Keys: the run uses the acting person's **own key** when they have one set
and enabled, else the **instance key**. The key source is recorded per
turn.

## 5. People, admins and flags

- **Roles.** `owner` (exactly one; on hosted the `OWNER_GITHUB` account,
  locally the daemon owner) is always an admin. `admin` is a role the
  owner grants to or removes from any connected person. Only the owner
  grants or removes admin; the owner cannot be demoted.
- **Admins may:** set per-person flags, set weekly credits and lifts, view
  all usage and audit, edit cost lines and margins, replace or clear the
  instance key, change provider endpoint and models, test the connection.
- **Flags** (per person, generic table, known names validated):
  `claxbot` (may use ClaxBot; default off except the owner, on) and
  `claxbot_unenforced` (meter, never block; default off).
- Everyone else sees only their own usage, keys, runs and audit events.

## 6. Code mode

### 6.1 Sandbox contract (`clax_bot_sandbox::Sandbox`)

`run(script, host, limits, cancel) -> ScriptOutcome {value, logs, calls,
error, cpu_us, mem_peak_bytes, wall_us}` (`clax_bot_sandbox` holds the
exact contract; billing charges memory at the larger of the configured
limit and the measured peak (`mem_peak_bytes`), and a
cancelled run still returns what it used). The script sees one global, `clax`,
whose functions are async and resolve through **host calls**:
`(op: &str, args: serde_json::Value) -> Result<Value, ApiError>`. No
`fetch`, no timers beyond microtasks, no module loading, no `eval` of
host strings. Limits (config): CPU 2 s, memory 64 MiB, 200 host calls,
logs 32 KiB, result 64 KiB. Exceeding a limit ends the script with a
typed error the model sees.

- **Native:** `clax-bot-js` (QuickJS via `rquickjs`), one runtime per
  script, interrupt handler enforcing the CPU budget (measured with the
  thread CPU clock where available, else wall time), `set_memory_limit`,
  peak memory from the runtime's usage report.
- **Worker:** a Cloudflare **Worker Loader** dynamic isolate
  (`worker_loaders` binding `LOADER`, `globalOutbound: null`), whose only
  binding is an RPC entrypoint back into the instance that performs host
  calls. CPU time from the isolate's reported CPU where available, else
  wall time; memory charged at the larger of the configured limit and the
  isolate's measured peak (its real limit is 128 MiB, so it may hold more
  than the configured limit).
- A shared **conformance suite** of scripts (in `clax-bot-sandbox`, run against
  both implementations) pins identical observable behaviour: results,
  errors, limits, call ordering.

### 6.2 The `clax` API (v0)

Defined once in Rust (`clax_bot::api`), each op with a JSON schema for
args and result, generating the `.d.ts` in the prompt. Every op executes
as the acting person under their rights, through the same store and
route code paths a person's own agent uses (no new write paths that skip
rights, publish checks or audit):

- `artifact.get()`, `artifacts.list({limit})`, `artifacts.create({title, description})` (only when the run's request allows)
- `files.list()`, `files.read(path)`, `files.write(path, text)`, `files.replace(path, old, new)`, `files.delete(path)` — on the run's **draft**
- `draft.check()` — static checks: HTML parses, page-contract lint
  (`<title>`, tokens, dark-mode blocks, body background, viewport), inline
  script syntax (parsed by the sandbox), references resolve
- `draft.publish({note, addresses})` — publishes the draft as a new
  version (at most once per step; the run's final publish)
- `db.collections()`, `db.list(collection, opts)`, `db.get(collection, id)`, `db.query(collection, q)`, `db.set/update/delete(...)` with version pins
- `threads.list({include_resolved})`, `threads.read(id)`, `threads.reply(id, markdown)`, `threads.resolve(id)`
- `console.log(...)`

### 6.3 Drafts

Each run edits a draft of its artifact: a copy of the current version's
files held in the run's state. `files.*` change the draft; the artifact
changes only on `draft.publish`, which publishes with `if_version` = the
version the draft was based on (a conflict surfaces to the model, which
may re-read and retry). A run that stops on limits keeps its draft,
viewable in the run panel ("Open the draft"), unpublished.

### 6.4 Entry points

- **Send to ClaxBot.** ClaxBot is listed by `GET /api/agents` as a
  built-in agent (handle `claxbot`, name `ClaxBot`, `builtin: true`,
  `live: true` when the caller has the flag and credits or unenforced;
  otherwise listed with `unavailable: "no_flag" | "no_credits" |
  "not_configured"`). Sending a thread (or a batch) to `claxbot` creates
  a run acting for the sender. Later comments on a ClaxBot-owned thread
  start a follow-up run on the same thread.
- **New artifact.** `POST /api/artifacts:new` gains `build: "claxbot"`:
  create the placeholder and brief as today, then send the brief thread to
  ClaxBot. The gallery shows a **New artifact tile** first in the grid
  that opens the existing dialog (`web/shell/src/ui/NewArtifactDialog.svelte`)
  with name, description and the "ClaxBot builds it now" toggle (on by
  default when available; off = today's "Who builds it?" step).

### 6.5 Replies

ClaxBot's replies and resolves are authored as agent `ClaxBot` acting for
the person ("ClaxBot (for Sam)"), go through the existing thread code, and
appear wherever agents' replies do. While a run is active its threads show
ClaxBot as working, via the existing working-state mechanism.

### 6.6 Rendering

No headless browser in v0: `draft.check` is static. Recorded as a known
gap (a later phase may use Cloudflare Browser Rendering on hosted and a
local headless browser).

## 7. Credits

### 7.1 Units

Stored as integer **milli-credits** (i64). Money as integer **micro-USD**.
Margins as integer **basis points**. No floats in the ledger.

### 7.2 Cost lines

Table of cost lines, each: `id` (stable slug), `label`, `unit`
(`mtok_input`, `mtok_cached_input`, `mtok_cache_write`, `mtok_output`, `cpu_gb_s`,
`mreq`, `gb_month`, `run`), `cost_micro_usd` per unit, `margin_bp`,
`applies` (`instance_key` | `always`), `source` (free text: where the cost
figure comes from), optional `model` (for token lines). Credit price
(micro-USD per credit) is an instance setting (default 10_000 = $0.01).
**A usage row's credits = qty × cost × (1 + margin) ÷ (credit price × the
unit's base quantity)**, computed exactly in integers and rounded up to the
milli-credit once per row (a fixed-credit line: qty × fixed ÷ base, rounded
up); the per-unit rate is shown rounded up but never charged from, and the
row snapshots the exact inputs. Seeded lines: input/cached/cache-write (5-minute writes, 1.25 × input)/output per model for the
current Claude models at list prices, script compute, DO requests,
Toolpath storage, a per-run harness fee (cost 0, fixed credits). Default
margin 40 % on tokens, 90 % on infrastructure (admin-editable). Changes
apply to new usage only.

### 7.3 Usage ledger

Every metered quantity writes one **usage row**: `person`, `run`,
`step`, `line`, `qty` (integer in the line's base unit: tokens, CPU·MiB·ms,
requests, bytes·s, runs), `cost_micro_usd`, `credits_milli`, `key_source`
(`instance` | `own`), `free` (bool: drawn from the free allowance), `at`.
Rates are snapshotted on the row; history never reprices. Tokens on a
person's own key write rows with `cost_micro_usd = 0` and
`credits_milli = 0` (visible, not charged); `always` lines charge
regardless. A failed model call charges nothing.

### 7.4 Weekly allowance, lifts and enforcement

- Week: Monday 00:00 UTC to the next. Injected clock everywhere.
- Each person's weekly credits = instance free tier (default 2,000) +
  their **lift** (admin-set, persists until changed). Spent = sum of the
  week's `credits_milli`.
- Before each step: if spent ≥ weekly credits and the person is not
  `claxbot_unenforced`, the run stops (`stopped`, reason
  `credits_exhausted`) and replies on its thread: what it did, that the
  draft is saved unpublished, and when credits reset. A new run is
  refused at send time with `credits_exhausted` (before any model call).
- Per-run cap (config) stops a runaway run the same way (`run_cap`).
- Warn state at ≥ 80 % (UI only).

## 8. Configuration (figment)

`HomeConfig` becomes a figment extraction: defaults, then
`<home>/config.toml`, then environment `CLAX_*` (double underscore for
nesting, e.g. `CLAX_BOT__MODELS__DEFAULT`), with figment metadata kept so
each value reports its **source** (`default`, `file`, `env:NAME`).
Behaviour preserved: an invalid table turns its feature off and the daemon
still starts (`clax doctor` warns); unknown keys in `[serve]` are logged
and ignored; the top-level `bin` line stays exactly where and as
`clax bin set` writes it (the plugins' wrapper reads it without a TOML
parser). New table:

```toml
[bot]
enabled = true                  # the instance may run ClaxBot at all
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com"
max_tokens = 32000
max_steps = 40
max_run_credits = 200
max_run_minutes = 30
concurrency = 4
free_weekly_credits = 2000
credit_price_micro_usd = 10000

[bot.models]
default = "claude-sonnet-5-5"
quick = "claude-haiku-5-5"
complex = "claude-opus-5-5"

[bot.script]
cpu_ms = 2000
memory_mib = 64
max_calls = 200
```

Instance settings edited in the UI (provider, models, cost lines, free
tier) live in the database (so the hosted Worker, which has no config
file, uses the same code); locally, a value set in `config.toml` or the
environment **overrides** the database value and shows read-only in the
settings screen with its source. `PUT /api/settings` writes `config.toml`
for file-backed fields with `toml_edit` (comments and order preserved).

### 8.1 Secrets

The instance key and people's own keys are stored **sealed**
(XChaCha20-Poly1305) in the database. The key-encryption key: locally
`<home>/secrets/kek` (32 random bytes, 0600, created on first use);
hosted the Worker secret `CLAX_KEK` (deploy.mjs generates and uploads
it like `SETUP_SECRET`). A key is write-only through every API: responses
carry only `{set, last4, set_at, set_by, last_ok_at}`. It is sent only to
the configured `base_url`. A key from `api_key_env` (local) takes
precedence over a stored instance key while the `base_url` in effect is
the owner's (default, file or environment); with one an admin stored it is
never sent, and only a stored instance key is used. `bot.base_url` in
`config.toml` is the owner's to write.

## 9. Toolpath and audit

### 9.1 New kinds

`docs/contract.md` "Toolpath journal and export" changes: besides writes,
these **reads and views** are recorded. New kinds (each a `structural`
change `clax.<kind>`; bodies never carry secrets or comment text beyond
what existing kinds carry; script code is stored by `sha256` with the
code itself in the run's step record):

- Bot: `bot.run_start {run, request (thread|brief), thread_ids, model, key_source}`, `bot.turn {run, step, model, input_tokens, cached_tokens, output_tokens, stop_reason, credits_milli}`, `bot.script {run, step, title, code_sha256, calls[], cpu_ms, mem_peak, credits_milli, error?}`, `bot.call {run, step, op, target, ok}` (every API op, reads included), `bot.run_end {run, outcome (finished|gave_up|stopped), reason?, credits_milli}`, `bot.refused {person, reason (no_flag|credits_exhausted|not_configured|model_not_priced), artifact?}` (a send to ClaxBot or a new artifact with `build: "claxbot"` refused before any model call; the admin pages' refused counts come from these).
- Views: `view.run {run}`, `view.admin {page, subject_person?}`, `view.audit {filters}`, `view.usage {subject_person}`.
- Admin: `admin.role {person, role, op}`, `admin.flag {person, flag, value}`, `admin.credits {person, lift_from, lift_to, reason}`, `admin.provider {fields[]}`, `admin.key {scope (instance|own), op (set|clear), last4}`, `admin.cost_line {line, fields}`.
- Actor is the person (or `agent:claxbot` acting for a person, carried as `on_behalf_of`).

### 9.2 Audit UI

`/audit` (linked from the admin pages, the run panel and People): reads
only `audit_events`. Filters: text, actor (person or ClaxBot), kind group
(views, changes, ClaxBot runs, credits & limits, admin), artifact, time
range; cursor pagination by `seq`; event detail shows the stored body and
links to the run and the artifact; "Export Toolpath" uses the existing
export. Admins see all events; others see events they acted in, on
artifacts they own, and runs acting for them. Opening the audit view is
itself a `view.audit` event.

## 10. HTTP API (new routes)

All JSON. Errors `{error: {code, message}}`. Browser routes follow the
existing SameOrigin/token/cookie rules (`docs/contract.md` "Security
model"); agents get 403 `not_for_agents` on every admin, settings, key and
audit route.

- **People** are named by their **person key** (`crates/clax-store/src/roles.rs`): a connected person's public ID, or `*owner` for the owner. Every `person` field below is one, and a route that names a person in its path takes it URL-encoded (`/api/admin/people/%2Aowner/flags`). **Actors** (`actor`, `on_behalf_of`, a key's `set_by`) are resolved by the route to `{id, name}`: a person key with the person's display name (`*owner` with the owner's name), `agent:claxbot` as ClaxBot, another agent's handle with its name, and `*system` as "Clax". **Key sources** are `"instance"|"own"`, never null: a field is null only where no run exists to have one.
- `GET /api/me/bot` → `{enabled, unenforced, available, unavailable_reason?: "no_flag"|"no_credits"|"not_configured"|"model_not_priced", role: "owner"|"admin"|"member", can_export, estimate_milli: {low, high}|null, provider_host: string|null, key: {own: {set,last4,set_at,set_by,last_ok_at,host,problem}|null, use_own}, week: {start, end, weekly_milli, lift_milli, spent_milli, warn, exhausted}}`. `provider_host` is the origin (scheme, host, port) of the provider in effect, which a key set now is bound to and only ever sent to (§8.1); a stored key's `host` is the one it was set for, and `problem` says when that is not the provider in effect (`your key: it was set for <host>; set it again`), when ClaxBot is unavailable `not_configured` for the person. `weekly_milli` is the whole allowance (free tier plus lift). `role` is the caller's (the shell shows Admin from it); `can_export` whether `GET /api/toolpath/export` admits the caller (the audit page's Export Toolpath); `estimate_milli` the typical credits of one run for the caller, the New dialog's line: the 25th to 75th percentile of the credits of their last 20 finished runs, else of the instance's recent finished runs, else a fixed default range from the settings.
- `PUT /api/me/bot/key {key}` / `DELETE /api/me/bot/key` / `PUT /api/me/bot {use_own}` → the same as `GET /api/me/bot`
- `GET /api/me/usage?week=<start>` → `{week: {start, end}, runs: [{id, artifact_id, artifact_title, started_at, status, scripts, cpu_gb_s, tokens, key_source, credits_milli, lines: [{line, label, unit, qty, credits_milli}]}], totals: [{line, label, unit, qty, credits_milli}]}`
- `GET /api/admin/people` → `[{person, name, role, flags{claxbot, claxbot_unenforced}, key_source, week{weekly_milli, lift_milli, spent_milli}, tokens_week, runs_week, refused_week}]` (admins; a bare array). `key_source` is the key the person's next run would use. `refused_week` counts the week's `bot.refused` events for the person (§9.1).
- `PUT /api/admin/people/<person>/flags {claxbot?, claxbot_unenforced?}` (admins) → the person
- `PUT /api/admin/people/<person>/role {role: "admin"|"member"}` (owner only → else 403 `owner_only`) → the person
- `PUT /api/admin/people/<person>/credits {lift_milli, reason}` (admins) → the person
- `GET/PUT /api/admin/provider` → `{provider:"anthropic", base_url, models{default,quick,complex}, key{set,last4,set_at,set_by,last_ok_at}, caps{max_steps, max_run_credits, max_run_minutes, max_tokens, concurrency}, sources{field: "default"|"db"|"file"|"env:NAME"}}`; `sources` keys are dotted (`base_url`, `key`, `models.default`, `caps.max_steps`); `PUT` takes any of `{base_url, models{…}, caps{…}}`
- `PUT /api/admin/provider/key {key}` / `DELETE …/key` (admins) → the provider
- `POST /api/admin/provider/test` → `{ok, latency_ms, error?}`
- `GET /api/admin/provider/models` → `{models:[{id, display_name}], fetched_at}` (from the provider's `GET /v1/models` with the effective key and URL)
- `GET/PUT /api/admin/pricing` → `{credit_price_micro_usd, free_weekly_milli, lines:[{id, label, unit, cost_micro_usd, margin_bp, fixed_credits_milli, applies, source, model?}]}`; `unit` one of `mtok_input`, `mtok_cached_input`, `mtok_cache_write`, `mtok_output`, `cpu_gb_s`, `mreq`, `gb_month`, `run`; `fixed_credits_milli` (null for most) charges fixed credits per unit, the harness fee; `PUT` takes the whole object
- `GET /api/admin/usage?week=` → `{week: {start, end}, paid_milli, free_milli, runs, refused, revenue_micro_usd, cost_micro_usd, free_cost_micro_usd, by_person: [{person, name, credits_milli, free_milli, cost_micro_usd, runs, tokens}], by_line: [{line, label, unit, qty, credits_milli, cost_micro_usd}]}`: `refused` counts the week's `bot.refused` events; revenue is paid credits × price, `cost_micro_usd` what the paid credits cost, `free_cost_micro_usd` what the free tier cost; the gross margin before and after the free tier follows from them
- `GET /api/settings` / `PUT /api/settings` (local daemon, the owner; `crates/clax-server/src/routes/settings.rs`): `GET` → `{file, fields: [{key, value, source, pending?, editable, restart_required, env, note?}], problems: [{table, message}], started_with_invalid_config, start_error}` (`value` in effect, `pending` what the next start takes); `PUT {set: {<key>: value|null}}` → the same plus `{written: [key], restart_required}`. An admin who is not the owner sees and sets only `bot.*` and `questions.*` fields (else 403 `owner_only`; such fields read `editable: false`): `bin`, every `*_env` field, `serve.*`, `sample.*`, `hosted.*` and `toolpath.*` stay the owner's, and so does `bot.base_url` (admins set the URL on the provider page).
- `POST /api/bot/runs {thread_ids, artifact_id}` (used by the send path) → `{run}`; `GET /api/bot/runs/<id>` → run with steps: `{id, artifact_id, thread_ids, request: "thread"|"brief", status: "queued"|"running"|"stopping"|"finished"|"gave_up"|"stopped", reason?, model, key_source, acting_for: {id, name} (the person key and name of the person it acts for), started_at, ended_at?, credits_milli, tokens?: {input, cached, output}, draft?: {url}|null (`/api/bot/runs/<id>/draft/`, served read-only and sandboxed to the person it acts for and admins), last_event_id, steps: [{n, kind, status: "running"|"done", started_at, ended_at?, credits_milli, text?, model?, input_tokens?, cached_tokens?, output_tokens?, title?, code?, calls?: [{op, target?, ok, ms, error?}], logs?, value?, error?, cpu_ms?, summary?, reason?}]}`; `POST /api/bot/runs/<id>/cancel` → `{run}`; `GET /api/bot/runs?artifact=` → `{runs: [run without steps]}`
- **One active run per thread.** `POST /api/bot/runs` or a thread's send to ClaxBot while a run that has not ended names any of its threads → 409 `{error: {code: "run_active", message, run: {id}}}`: no second run starts and nothing is charged; the web opens that run. A comment on a ClaxBot thread from a person or the owner who may send (`may_send_at`) starts a follow-up run acting for its author, on their credits and rights, in which only their comments are `from_sender`; the comment's answer carries `run: {id}`. A comment made while a run works on the thread answers `run: {id, queued: true}` (the active run) and queues its author for the thread (`bot_follow_ups`), once each, in arrival order; each run's end starts the next queued person's run, acting for them alone (a person who may no longer send is dropped, a refusal recorded as `bot.refused` and the next tried), so several people's words never share one run. A person is queued in the transaction that finds the run still working (else their run starts at once), stays queued until their run is made, and a driver starting starts the follow-ups of threads no run works on (a crash between a run's end and its follow-ups); a deleted or resolved thread's queue is dropped. Agents' comments start none.
- **Who sees a run.** The person it acts for and admins see it whole. Someone who only reads its artifact sees the outline: the run's summary and each step's place, kind, status, times, credits, a script's title and its calls' ops, `ok` and `ms`, a finish's summary, an end's reason; never `text`, `code`, `value`, `logs`, `error`, tokens or a call's `target`, on `GET` and over the events (no `turn_delta`; `script` without `code`). The events stream checks access again at every `step_done` and `run_end` and after three minutes of quiet, and ends once the reader may no longer see the run.
- **An agent's run.** A run an agent sends is bounded by the agent's grant: every write it makes, and every read, takes the person's rights narrowed by the grant (`bot_runs.agent_grant`), and stops once the grant ends.
- `GET /api/bot/runs/<id>/events` (SSE: `step_started {step}`, `turn_delta {step, type, text}`, `type` one of `text`, `thinking` (the reasoning summary) or `tool_input` (the tool call's input JSON as written), `script {step, title, code}`, `call {step, call}`, `step_done {step, run?}`, `run_end {run}`, each `data` JSON). Every event carries an SSE `id` of `<step>.<attempt>:<seq>`, all decimal: `attempt` counts the times the step was begun (a step begun but not committed before a crash is begun again with the next attempt, which the run loop persists), so no ID repeats within a run; IDs order by (step, attempt, seq). A request with `Last-Event-ID` replays every event after that ID and then the live ones; without it, from the run's first event. After a restart the replay is rebuilt from the committed steps (each as `step_started` and `step_done` with its final content, no deltas) plus the live tail of the open attempt. A client opens the stream with the run's `last_event_id` and, on reconnect, the last ID it applied; it drops any event at or below it, and takes a `step_started` for a step it already holds as that step begun again, replacing the step's partial state.
- `POST /api/artifacts:new {title, brief, build?: "claxbot"}` with `build` → the usual answer plus `run: {id}`
- `GET /api/audit?q=&actor=&group=&artifact=&from=&to=&cursor=` → `{events:[{seq, at, actor: {id, name}, on_behalf_of: {id, name}|null, kind, summary, artifact_id, run}], next_cursor: number|null}`. `actor=` takes `claxbot` (ClaxBot acting for anyone: the store's `agent:claxbot`) or a person key (that person and their agents, ClaxBot left out); `group` one of `views`, `changes`, `bot`, `credits`, `admin`; `cursor` is the `next_cursor` of the page before, a `seq`. `GET /api/audit/<seq>` → the event with its stored `body`

## 11. Web UI

Svelte shell (`web/shell`), the existing theme tokens and components:

1. Gallery **New artifact tile** + dialog toggle (§6.4).
2. Send menu: a "Clax" group first with ClaxBot and its availability line.
3. **Run panel** beside the artifact: narration, each script (code,
   calls, output, collapsed except the live one), cost so far, Stop, Audit
   link, "Open the draft" when stopped with one.
4. Usage UX: meter in the top bar when enabled; warn banner at 80 %;
   exhausted banner; ClaxBot disabled in the Send menu with the reset
   date; the stopped-run reply.
5. **Admin** (`/admin`, admins only, linked from People): People & flags,
   Provider (key write-only, model selectors from the provider list with
   refresh and custom ID, test connection), Pricing (cost lines, margins,
   credit price, free tier), Usage (margin before/after free tier).
6. **Settings** (localhost): every config field with its source; env-set
   fields read-only; "Save to config.toml".
7. **You** (`/me`): ClaxBot usage this week, own key, runs list.
8. **Audit** (`/audit`, §9.2).

Phone width works (16 px gutter, no horizontal scroll); light and dark.

## 12. Verification

- Unit tests per crate; the sandbox conformance suite on both sandboxes;
  the run state machine against a scripted model (no network) including
  resume-after-crash at every step boundary, cancel, credits exhaustion,
  per-run cap, own key vs instance key.
- HTTP tests for every route's rights (owner, admin, member, agent,
  guest), key write-only-ness, Toolpath events for each view and op.
- Web unit tests and Playwright e2e for each screen against a daemon
  with a scripted model provider (a fake Messages API, as
  `scripts/fake-anthropic.py` does).
- Worker: `test-wasm.mjs`/`e2e.mjs` gain a ClaxBot run under
  `wrangler dev --local` with the Worker Loader sandbox and a fake model.
- Injected clocks; no sleeps; gates stay under two minutes.

## 13. Known gaps and risks (record as qualifier concerns)

No rendering (§6.6); Worker Loader CPU/memory measurement precision;
QuickJS CPU-clock fallback to wall time; seeded cost figures are list
prices to be checked; credit charge for DO requests and storage is
estimated per step, not read from Cloudflare billing.
