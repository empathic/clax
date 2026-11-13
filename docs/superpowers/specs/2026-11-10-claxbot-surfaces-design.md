# ClaxBot surfaces

A design for `crates/clax-bot`, the store tables under it and the daemon
routes that serve it. Nothing here is built. Three example files were
written to test the format: `shell.d.ts` (ClaxShell) and `review.d.ts`
(Changeling) stand beside it in `2026-11-10-claxbot-surfaces/`, and
`clax.d.ts` (today's operations) is now ClaxBot's own,
`crates/clax-bot/src/clax.d.ts`. A throwaway parser has read all three
("The parser spike").

## Intent

What a ClaxBot run can do, what it is told and what kinds of request it
takes are compiled into Clax: one set of operations
(`crates/clax-bot/src/clax.d.ts` since step 1; before it, the table
`OPS` of `crates/clax-bot/src/api.rs`), one prompt
(`crates/clax-bot/src/prompt.rs`), two request kinds (`RequestKind`), and
a run loop that knows about artifacts, threads, drafts and replies. A
product that hosts ClaxBot can add nothing. ClaxShell works around this by
telling ClaxBot, in a comment's words, to write plans into a database
document that its host polls. Changeling, whose tools are answered by a
browser tab and whose jobs have no artifact, could not use ClaxBot at all.

A **surface** is one TypeScript declaration file that says what a host
gives a run. ClaxBot reads the file, shows it to the model, checks every
call against it, journals and meters the calls, and hands each one to
whoever provides the surface. Everything Clax itself offers a run
(artifacts, threads, drafts, web tools) is a surface too, named `clax`,
read and dispatched by the same code.

The measure of success: ClaxShell adds a `shell` surface to its daemon by
writing one file, setting configuration and speaking one documented HTTP
protocol, with no change to this repository.

## Four ideas

1. **The core knows no nouns.** The run loop knows runs, steps, scripts,
   a journal, a meter and content. Artifacts, threads, drafts and replies
   belong to the `clax` surface. A run with no `clax` surface is legal.
2. **A kind is a function.** A request kind is declared as a function
   from a typed request to a typed result. A run is one call of one
   kind. `finish` takes the kind's result, checked like any call, and
   the surface that declared the kind receives it when the run ends. A
   kind may also say what messages it takes after it starts; a run is
   stored state, and what is appended to it, and when, is for whoever
   drives it to decide.
3. **A call can wait.** A script awaits a call as it awaits any other,
   however long the answer takes. While it waits the run parks, holding
   no driver and no sandbox, and when the answer comes the script goes
   on from where it was.
4. **Policy is the environment's.** ClaxBot is a mechanism. How long a
   run may go, how much it may spend, how much history the model is sent
   and how it is compacted, and what the prompt says are supplied by
   what runs it (a daemon, the Worker, a host's configuration), never
   compiled in. Nothing inherent to ClaxBot caps a run.

| What was asked for | Follows from |
|---|---|
| Runs with no artifact (Changeling) | 1 |
| A host's own request kinds, with results as data | 2 |
| Structured comments, a quest log's steps | 2: a kind's typed result; operations for the rest |
| Events that continue work | 2: a message to the run, or a new run; the environment chooses |
| Questions to the person | 3 |
| Confirmation before a host's effect | 3: the operation waits while the host asks |
| Waiting on child runs | 3 |
| An outside agent doing a long job | 3 |
| A host's own persona and prompt | 2: the kind's surface owns the prompt |
| A session of any length; compaction | 4 |

## Not in this design

- Hosted providers: a browser tab answering a hosted Clax and how it
  signs in.
- A page registering a surface and running ClaxBot for its viewers.
- Tenants, resale, usage and history per end user, isolation guarantees.
- A Clax artifact nested in another product's review. It needs the
  review's person to be a person in Clax, which is the accounts project.
- Two bindings of one surface in a run (two artifacts at once).
- A host confirming Clax's own `draft.publish`.
- A compacting history policy, sessions and working memory. The seams
  for them are here ("The environment's policy"); the policies are not.
- Rebuilding a run's exact context, and outcome cases and evals ("From
  the Anvil review" says what each needs from this design).
- Thread runs seeing a comment's clip: a change of its own to `clax`,
  after content.
- A general-purpose agent in the manner of Claude Code. It is not a
  goal; "What is no longer in its way" lists what this design removes.

## What would make this a failure

- A second tool-calling protocol beside code mode.
- A surface that widens what a person may see or change in Clax. Only
  `clax` touches Clax's data, as the run's person.
- A prompt or declaration that changes during a run.
- A run whose abilities depend on timing.
- A limit or a history rule compiled into ClaxBot.
- Any change a person on clax.dev can see, from steps 1 and 2.
- `clax` keeping any path a host's surface cannot take.
- Two ways to do one thing in the host protocol.

## The surface file

A surface's definition has two notations, which say the same thing and
convert both ways (`clax bot surface convert`): a `.d.ts` file, for a
person to write and read, and a JSON manifest, for a program to send
when it registers ("Two notations"). This section shows the first.

A surface is a `.d.ts` file in a small subset of TypeScript: what
`api::dts` prints today, plus `extends`, doc comments and JSDoc tags.
ClaxBot parses it with a parser of its own (no dependency; it builds for
wasm32). It prints a definition two ways: for tools, as a file again
(printing a parsed file and parsing it gives the same definition); for
the model, in the wording `api::dts` uses today, so the declaration a
clax.dev run reads does not change by a byte.

```ts
/**
 * A part of the shell must call `claxos.call('shown')` once it has drawn.
 */
declare const shell: {
  /** The windows open on the desktop. @read */
  windows(): Promise<Window[]>;
  /** A part or a window as it draws now. @read @throws not_found */
  screenshot(of: { part: string } | { window: string }): Promise<Image>;
  settings: {
    /** Changes appearance settings. @write @throws invalid */
    set(patch: SettingsPatch): Promise<Settings>;
  };
  window: {
    /** Opens an app's window on the person's screen. @effect @throws not_found */
    open(artifact: string): Promise<Window>;
  };
  /** Asks the person in the Comment box. @read @waits @throws declined */
  ask(question: string, choices?: string[]): Promise<string>;
};

interface Kinds {
  /**
   * The request is a comment the person drew across several apps. Decide
   * what each app should be asked, and ask each with `clax.threads.open`.
   * @allow shell.windows shell.screenshot shell.ask threads.open runs.wait
   */
  triage(request: Triage): TriageResult;
}
interface Triage { ask: Instruction; touches: Touched[]; clip?: Image }
```

**One `declare const <name>`** is the surface: its name (one lowercase
word) and its operations, nested as namespaces. A script reaches every
surface through the one global it has today, `clax`: the file above
gives a run `clax.shell.windows()`. Clax's own operations keep their
names (`clax.files.read`), and a host's name may not be one of Clax's
namespaces: registering `files` or `threads` is refused.

**Rules.** The const's doc comment is added to the system prompt under
`# <name>`. A namespace's doc comment is rules too, included only in runs
that have one of the namespace's operations. Rules are the operator's
words; everything a provider sends at run time is data.

**Operations.** Each has a doc comment (its documentation), exactly one
access tag and optional others. A tag the format does not know is an
error naming the line.

| Tag | Means | What Clax does |
|---|---|---|
| `@read` | No effect | Journals the answer |
| `@write` | Changes the provider's state | Sends the journal key; the provider applies a repeated key once and answers the same |
| `@effect` | The person sees it, or it leaves the machine | As a write, and counted against a limit per run |
| `@spend` | Charged each time (`api::Access::Spend`) | `clax` only; a host's file using it is refused |
| `@waits` | May take as long as a person does | Tells the policy, which by default gives it the long deadline, and tells the model |
| `@once` | Repeating it is not safe | Never sent again once a provider has accepted it |
| `@throws a` | An error code it may fail with; one code a tag, repeated for more | Printed for the model |

**Kinds.** `interface Kinds` holds one method per kind:
`name(request: Req): Result`, or `name(request: Req, message?: Msg): Result`
for a kind that takes messages after it starts ("A run's inbox"). Its doc comment is the prompt's paragraph
about the request. `@allow` lists the operations a run of that kind has,
as `surface.op` or `surface.namespace.*` patterns.

**Types** are those `clax_surface::Ty` has, which the operation table's
`api::Ty` had before it (`string`, `number`, `boolean`, `null`,
`unknown`, string-literal unions, `true` and `false`, arrays, object
types with unknown fields refused, `Record<string, T>`, unions, named `interface`s, which
may `extend` others), doc comments on fields, and four the core declares:

| Type | Is |
|---|---|
| `Int` | A whole number |
| `Instruction` | The run's person's own words. Allowed in two places: inside a kind's request, and as the result of an `@waits` operation (the person answering, or saying what to do next). Anywhere else a file that uses it is refused. Every other string is data |
| `Content` | A handle to stored bytes (below) |
| `Image` | A `Content` that is an image |

Anything else is an error naming the line. A range a type cannot say
(`limit` from 1 to 100) is the provider's to check and refuse.

**Limits,** each a default the environment may change: a file 64 KiB,
rules 4 KiB, a request 64 KiB, a call's result 1 MiB, eight surfaces a
run. Reserved names: `clax` and each of its namespaces, `runs` and
`content` among them.

**What the model reads** is the run's part of the file in today's
wording: the operations the run has, and only the interfaces they name.
`Kinds` is not printed.

| In the file | The model reads |
|---|---|
| `@read` | Nothing more |
| `@write`, `@effect` | "(writes)" |
| `@spend` | "(charged)" |
| `@waits` | "(may take long)" |
| `@throws a @throws b` | "Errors: a, b." |
| `Int` | `number` |

## Two notations

| | `.d.ts` | JSON manifest |
|---|---|---|
| Written by | A person, by hand | A program: from an MCP server's tool list, or a host's own types (`schemars`, zod, pydantic) |
| Types | The subset above | The same subset as JSON Schema: `type`, `enum`, `const`, `items`, `properties` with `required` and `additionalProperties: false`, `additionalProperties` as a schema, `anyOf`, `$ref` to the manifest's own types, `description`, and `$ref: "clax:Content"`, `clax:Image`, `clax:Instruction`, `clax:Int` |
| Tags | JSDoc | Fields: `access`, `waits`, `throws`, `allow` |
| Good at | Reading; review in a diff; the host's handlers type-checked against it | Being generated and sent; MCP tools map onto it one for one |
| Weak at | Being generated; a parser to maintain | Reading; a schema outside the subset is refused, so "any JSON Schema" is not accepted |

Both parse to one definition, and everything after that is one path:
the checks, what the model reads, a definition's hash (over its
canonical bytes, `clax_surface::canonical_manifest`). The manifest's
shape is the one `crates/clax-surface`'s documentation gives ("The
manifest notation"). It began from the one
`../claxos/docs/claxbot-services.md` §1 proposed and differs from it in
names and in what it holds: `clax_surface` is the notation's version
and `name` the surface's (for `service` and `version`); `operations`
with `description`, `access`, `params` and `result` (for `ops` with
`doc`, `kind`, `args` and `returns`), and with `waits`, `once`,
`throws` and the entries of namespaces that have rules or hold
nothing; `interfaces` (for `types`), since only an interface has a
name, with `extends`; and `kinds`. The reader and the writer of the
subset are `crates/clax-surface/src/manifest.rs`.

Why both, where an earlier draft had `.d.ts` alone: registration at run
time ("Registration and authority") needs a form a program builds and
sends, and a page's script or an MCP bridge has JSON, not a
TypeScript file. Why not JSON alone: a person reviewing what a host may
do reads forty lines of TypeScript far more surely than four hundred of
schema, and the model reads TypeScript in either case. The cost is a
second reader to keep exactly equal to the first; a test converts every
example file both ways and compares.

Not decided: whether constraints JSON Schema can say and TypeScript
cannot (`minimum`, `maxLength`, `pattern`) join the definition, written
in `.d.ts` as tags on a field. They are refused for now.

## A run

A run is: a person, a model, a budget, one **request**
(`surface.kind`, its payload) and a set of **bindings**. A binding is a
surface, the SHA-256 of its file, a **scope** (the provider's own value:
`{"artifact": "…"}` for `clax`, a subject key for a review) and the
operations the run has. All of it is fixed when the run is made; the
files' text is stored once by hash. A resumed run builds the same prompt
against the same declaration.

**Which surfaces a run has** is decided when it is made, from what is
already known, and asks no one:

| Surface | Bound when |
|---|---|
| The one that declared the kind | Always |
| One the starter names | The start says `bind: {"clax": {"artifact": "…"}}`, checked as the person |
| One with a standing join | The host said beforehand which scopes of which surface it joins (`shell` joins runs whose `clax` scope is one of its parts) |
| `content`, `runs` | Always |

A bound surface whose provider is not connected stays bound; its
operations answer `unavailable`.

`Run::artifact_id`, `Run::thread_ids`, `Run::base_version`,
`Run::request` and `Run::web` leave the core's view of a run. They are
the `clax` binding's scope, the request's payload and the binding's
operations. The run loop reads none of them, and `bot_runs.artifact_id`
becomes optional.

**The operations a run has** are an intersection, computed once:

    the file's  ∩  the operator's `allow`  ∩  the starter's `allow`
                ∩  the kind's `@allow`     ∩  the agent grant's, if any

An operation outside it is not declared and is refused when called. The
web grant is this rule applied to `clax.web.*`. What a provider may
refuse later (a grant the person narrowed mid-run) it refuses itself,
with `forbidden`.

**Who may read a run.** Today someone who only reads a run's artifact
sees each step's place, kind and state, a script's title, its calls'
operations and outcomes, and the finish's summary; never code, values,
logs, errors or targets (`for_reader`,
`crates/clax-server/src/routes/bot.rs`). That stays for a run with
`clax` alone. A host's operations, and a title a model wrote after
reading a host's data, may say more than a reader should know, so each
registration says what a reader may see of it: `reader_view = "none"`
(the default) or `"names"`. A run shows a reader the least any of its
bindings allows: with one at `"none"`, that a run happened, its kind,
its state and times, and the finish's summary, which is the reply
readers of the artifact see anyway. The request, arguments, results and
content are always the run's person's only. A reader is
defined through the artifact (`run_access`, `reads_artifact`), so a run
with no `clax` binding has none: its person and the admins see it, it is
listed under the person's runs and under the surface that started it,
and its audit records are filed on the person.

## Providers

`api::Performer` goes. The run loop holds one provider per binding:

```rust
trait SurfaceProvider {
    async fn read(&self, cx: &OpCtx<'_>, op: &str, args: &[Value]) -> Result<Value, HostError>;
    /// `@write` and `@effect`. `cx.key` is the journal key.
    async fn write(&self, cx: &OpCtx<'_>, op: &str, args: &[Value]) -> Result<Value, HostError>;
    /// A run this surface is bound to is ending: called inside the
    /// terminal step, with the claim, and answers what the step keeps.
    async fn ending(&self, cx: &OpCtx<'_>, end: &End) -> Result<Value, HostError>;
    /// A run this surface was bound to ended. `result` is the kind's
    /// result, given only to the surface that declared the kind.
    async fn ended(&self, run: &RunInfo, end: &End, result: Option<&Value>) -> Result<(), HostError>;
}
enum End { Finished, GaveUp { code: String, reason: String }, Stopped { reason: String }, Failed { code: String } }
```

- **Starting a run.** A provider starts a run of one of its own kinds:
  the kind, a request, a person, and what else to bind. Nothing turns a
  run of one kind into another.
- **`clax` provides** what exists: the draft, publishing, `BotHost`, the
  web source, and the kinds `thread` and `brief` with the rule that a
  thread run which finishes without replying has its summary posted. Its
  writes keep their journal entry in the write's own transaction
  (`WritePlan`).
- **The end of a run** goes to every binding, so `clax` posts its stop
  notices, clears the roster and starts queued follow-ups for any run
  that holds threads, whoever's kind it is. `Stopped` and `Failed` carry
  today's reasons (`credits_exhausted`, `run_cap`, …). The end is
  recorded with the run and delivered until the provider acknowledges
  it, for seven days.
- **Checks.** Arguments are checked against the file before a provider
  sees a call; results before the script does (`invalid_result`); a
  kind's request when the run is made; `finish`'s value against the
  kind's result (`invalid_result`, and the model may call `finish`
  again). A value that fails its check changes nothing.
- **An outside provider's write** is called with the key
  `<run>:<step>:<seq>` (`OpCtx::key`) and its answer is journaled when
  it arrives. A step run again after a crash finds no entry and sends
  the same key, which the provider treats as the same call.
- **Effects** past the surface's limit for the run are refused
  (`effect_limit`) before the provider sees them. A key counts once.
- **Deadlines and retries** are not ClaxBot's to fix: "Deadlines,
  retries and what a call may promise".

`FakeSurfaceProvider` and a conformance suite, beside `FakeBotHost` and
`host_conformance`, pin what a provider must do.

## Calls that wait

To a script, every call is the same: `const answer = await
clax.shell.ask(…)`, and the next line runs when there is an answer.
What differs is underneath.

1. The run loop sends the call with its key, `<run>:<step>:<seq>`
   (`OpCtx::key`), and its deadline.
2. Answered within `park_after_s` (default 10): the script goes on in
   the sandbox it is in.
3. Otherwise the step is abandoned uncommitted and the run **parks**: a
   status of its own, no driver, no sandbox, no charge.
4. The answer, or the deadline's error, is journaled when it arrives,
   whether or not the run is executing, and the run becomes due.
5. The step runs again. Its script is answered from the journal call by
   call, reaches the awaited call, gets its answer, and goes on.

Steps 3 to 5 are what a crash already does (a step begun and not
committed is executed again and answered from the journal, `host.rs`),
and scripts are already deterministic for it: a fixed clock, a seeded
`Math.random`, nothing reachable but `clax`. So a script cannot tell a
fast answer from a slow one, and no script is written differently
because a call might wait. A sandbox cannot be kept for a day, or
through a restart, or at all between a Worker's requests; replaying is
how the awaited value survives all three.

| Question about a parked run | Answer |
|---|---|
| Is it active (`active_on_in`, `crates/clax-store/src/bot_runs.rs`)? | Yes. It keeps its threads: a second send of a thread returns it, and follow-ups queue behind it |
| What does the roster show? | "Waiting on `<surface>`", not "working" |
| Does parked time count against `max_run_minutes`? | No. The cap, where there is one, counts time not parked |
| Is the resumed step checked for credits, the stop flag and caps? | Yes, as any step. The answer is journaled first, so the run's record keeps it |
| What does replaying cost, and who pays? | The script's own work up to the awaited call, again; its earlier calls are answered from the journal and reach no provider. The instance pays, as for a crashed turn. Only the committing attempt is charged |
| How often may one step park? | A setting (`max_parks_per_step`, default 8, or none) |
| What does cancelling do? | Ends the run; the provider learns from `ended` |
| How many runs may one person have parked? | A setting (`max_parked_runs`, default 20) |

`@waits` does not change what a script does. It gives a call the long
deadline, and tells the model the call may take long.

This is the whole mechanism for questions (`clax.shell.ask`, and a
`clax.ask` over Clax's own question UI), for a host that asks the person
before performing an effect, for `clax.runs.wait`, and for an outside
agent that takes a job and returns a patch.

## Deadlines, retries and what a call may promise

**ClaxBot holds no numbers.** How long a call may take, how long a run
may stay parked and how often a call is tried again come from the
environment's policy, for each run and each call
(`RunPolicy::delivery`), so a long-running agent and a quick edit on one
instance can differ, and one run can be given more time than another.
Every figure in this document (30 seconds, 24 hours, 10 seconds before
parking, 20 parked runs) is the daemon's default policy, not a property
of ClaxBot. A deadline may be absent.

**Each operation says what repeating it means,** because only its
surface knows:

| The operation is | Sending it again with the same key | So when no answer comes |
|---|---|---|
| `@read` | Harmless | The run loop sends it again, as the policy says |
| `@write`, `@effect` | Applied once: the contract of a key | The run loop sends it again with the same key, as the policy says |
| `@write @once`, `@effect @once` | Not safe: the provider cannot tell a repeat | It is not sent again once the provider has accepted it |

**Retrying is the run loop's work, not the model's.** A script awaits;
underneath, a call that fails on the way or gets no answer is sent again
with its key, on the schedule the policy gives (attempts, backoff, the
whole deadline), within what its surface declared. A provider keeps its
own account too: it may answer `retry_after`, or an error that says it
is final. Only when the policy is spent does the script see an error,
and the error says what is known:

| The script sees | Meaning |
|---|---|
| `unavailable` | No provider accepted the call. Nothing happened |
| `expired` | A read was accepted and not answered in time |
| `unknown_outcome` | A write or effect was accepted and not answered. It may have taken effect |

A provider says it has a call by answering `{"id": "…", "accepted":
true}` before its result; without that, Clax cannot tell the first row
from the third and reports the third for anything it sent.

**Room for durable guarantees.** What systems such as Temporal and
Cadence give, and what is here:

| They have | Here |
|---|---|
| Workflow code replayed deterministically from an event history | A step's script replayed from the journal ("Calls that wait") |
| Activities with idempotency keys, timeouts and retry policies | Operations with keys; deadlines and retries from policy, within what the operation declares |
| Signals | A run's inbox |
| Timers | Nothing yet. A core operation that waits until a time is a call that parks, and needs nothing new |
| Child workflows | Runs a run starts, with a parent link |
| Heartbeats from long activities | `accepted`; a heartbeat is the same message sent again, not specified here |
| Exactly-once effects in outside systems, compensation | Not here. They need the provider's part, which a key makes possible and does not supply |

None of the right-hand column is a promise this design makes beyond what
it says elsewhere. It is the claim that adding those guarantees later
changes policy and providers, not the run loop's shape.

## A run's inbox

A run is stored state: its request, its steps, its journal. What is
appended to it, and when, is not ClaxBot's to restrict.

- **A message** is a typed value sent to a run by the surface that
  declared its kind (`POST …/runs/<id>/messages`), of the type the kind
  gives as its second parameter. A kind with one parameter takes none.
- **It is delivered between steps,** never inside one: the model's next
  turn begins with it, in arrival order with anything else that woke the
  run. A message to a parked run wakes it; the call it was waiting on
  stays out, and its answer wakes the run again.
- **Whether a run accepts one is the environment's:**
  `RunPolicy::accepts(run, state)` answers accept, refuse, or start a
  new run of the same kind with it. An ended run may be continued if the
  policy says so; `ended` is then delivered again when it next ends.
- **`clax`'s kinds take no messages,** so a thread sent again while its
  run works queues a follow-up run, as it does today.

A message's strings are data unless typed `Instruction`, like a
request's.

## One door to the model

Nothing a provider sends reaches the model except through a script, or
as the run's own request and messages. There are four things a model is
shown, and each passes one function, `to_model`, with the step's journal
beside it:

| Shown to the model | Whose choice |
|---|---|
| The request, and any message | The starter's, checked against the kind |
| What a script returns | The script's |
| What a script logs (`console.log`) | The script's |
| An error a call failed with, and Clax's own account of the step (which calls, which outcomes, what is left) | Clax's; a provider's error message is its data |

A call's answer goes to the script and stops there. A script that reads
a web page, a comment or a diff and returns three lines of it has shown
the model three lines.

`to_model` is where a guardrail goes, and it is the environment's
(`RunPolicy`): it sees each value about to be shown, and the journal
says where every value that entered the sandbox came from (which
surface, which operation, typed `Instruction` or not). A policy can
therefore refuse or redact what came from a source it does not trust,
before the model reads it. The default lets everything through, which is
today's behaviour. None is built here; the door is.

An `Instruction` keeps its standing only when it is shown whole: a
string the journal recorded as an `Instruction`, returned unchanged, is
shown under the core's heading for the person's words. Changed in any
way, it is data.

## Content: images, files and their metadata

Bytes never travel inside a call. A value of type `Content` is a handle:

```ts
interface Content {
  id: string;            // names it within the run
  media_type: string;    // as Clax determined it
  bytes: Int;
  sha256: string;
  name?: string;         // a file name, if the provider gave one
  width?: Int;           // images: read from the file by Clax
  height?: Int;
  meta?: Record<string, unknown>;  // the provider's own, 4 KiB at most
}
type Image = Content;    // media_type image/png, image/jpeg, image/webp or image/gif
```

**The store.** A run's content is bytes by SHA-256 in the blob store
that holds artifact files and assets, on the daemon and on the Worker,
with a row per item naming the run and the surface, operation and key
that brought it. Content is pruned with the run's call journal
(`BOT_JOURNAL_KEEP` after the run ends); what a run put in a draft or a
thread lives on there. One item is 20 MiB at most and a run's 100 MiB,
or the blob store's own limit on a target where that is lower. The
Worker's limit is not verified here; step 4 starts by measuring it.

**How bytes get in.** One way only: the provider uploads, then answers
with the ID where the type says `Content`.

    PUT /api/bot/surfaces/shell/content?run=<run>&name=<name>
    Content-Type: image/png
    <bytes>
    → {"id": "…", "sha256": "…", "bytes": 48213}

In the answer the provider writes `{"id": "…", "meta": {…}}`; Clax fills
in the rest from the store. `media_type`, `bytes`, `sha256`, `width` and
`height` are Clax's findings, never the provider's claims: an upload
whose bytes are not what its `Content-Type` says is refused, and a value
typed `Image` that is not one of the four image types fails
`invalid_result`. A request's payload carries content the same way
(a comment's clip, the area a person drew).

**What the script can do with a handle:**

| To | How |
|---|---|
| Let the model see an image | Return the handle from the script, anywhere in its value |
| Read text content | `clax.content.text(c, {offset?, limit?})`: a page of it, as `web.fetch` pages |
| Hand it to another surface | Pass it as an argument typed `Content`: `clax.files.put(path, c)` puts a screenshot in the draft |
| Use it in a later script | `clax.content.get(id)` |

**Showing the model.** After a script step, each `Image` handle in the
returned value is attached to the tool result as an image, in order, with
the JSON naming which is which. At most four a step, each within the
model's own limits; one over them is left out and its handle gains
`"shown": false` with why. Runs go through `clax_sample`'s
`MessagesClient`, whose `Block` gains an image variant inside a tool
result. Every backend carries it the same way: the Anthropic client,
and the OpenAI and Gemini translators, which reduce a tool result to its
text today (`messages/openai.rs`, `messages/gemini.rs`) and are changed
in the content step to carry its images. Whether images are shown is
decided by what the model can take, never by which backend it is behind.
With a model that takes none, none are attached, every handle says
`"shown": false`, and the prompt says so; an image is never dropped
silently. Images in the request
are shown with the first message under the same limits.

**A provider reading content** it was passed:

    GET /api/bot/surfaces/shell/content/<id>?run=<run>

answers only for a handle that was an argument of a call of that run to
that surface.

**Metadata, in one place:**

| About | Where | Who says | Trusted |
|---|---|---|---|
| The bytes (type, size, hash, dimensions) | The handle | Clax | Yes |
| Their name | `name` | The provider | Data |
| Anything else about them (a path, a commit, a caption) | `meta` | The provider | Data |
| Where they came from (surface, operation, key, step) | The run's record | Clax | Yes |
| An operation (access, waits, errors) | The file's tags | The operator | Yes |
| Whose words a string is | The `Instruction` type, in a request or a waiting call's answer; every other string is data | The file, and the provider that fills it | As the provider is |
| A call (run, key, deadline, person) | The protocol | Clax | Yes |
| The prompt (each piece's name and hash) | The run's record | Clax | Yes |

`clax` gains `files.put`, which takes content. A thread's clip becoming
`clip?: Image` in the `thread` request is a later change of its own: it
changes what a clax.dev run is sent.

## Built-in surfaces

| Surface | Provided by | Holds |
|---|---|---|
| `clax` | Clax's server (`BotHost`) | Today's operations, the kinds `thread` and `brief`, and two new operations: `threads.open` and `ask` |
| `content` | The core | `text`, `get` |
| `runs` | The core | `wait` |

`crates/clax-bot/src/clax.d.ts` has replaced `OPS` and its `Ty` statics.
The declaration the model reads does not change: `api.d.ts.golden` and
`api.web.d.ts.golden` stand as they are.

**Fan-out** is a typed operation of the surface that owns the kind, and
one core operation to wait:

```ts
// in clax.d.ts
threads: {
  /** Opens a thread on another artifact the person may comment on, sent
      to ClaxBot: a child run. @write @throws forbidden @throws not_found @throws limit */
  open(artifact: string, text: string, anchor?: unknown): Promise<{ thread: string; run: string }>;
};

// a namespace of `clax`, provided by the core
declare const runs: {
  /** How each child run ended. @read @waits */
  wait(runs: string[]): Promise<RunEnd[]>;
};
interface RunEnd { run: string; state: "finished" | "gave_up" | "stopped" | "failed"; result?: unknown; reason?: string }
```

What ClaxBot fixes about a run starting a run is small: the new run
records which run started it, and `clax.runs.wait` takes the runs the
caller started. Everything else is the meaning of the operation that
starts it, and the environment's policy:

- `clax.threads.open` means: as the person, on an artifact they may
  comment on, spending from the caller's budget. That is this
  operation's meaning, chosen so that one comment costs one budget.
- Another operation may mean otherwise. A short run may start a long
  one with a budget and limits of its own, and finish without waiting
  for it. Whether a starter may do that, and who pays, is policy.
- How many runs a run may start and how deep they may go are settings,
  with no depth fixed by ClaxBot.

The sandbox does not change: it defines `clax` from a list of dotted
operation names (`clax_bot_sandbox::validate_ops`), and a host's
operations are more names in that list (`shell.windows`). The journal
and events name them the same way, so a run in flight at an upgrade
resumes.

## The prompt

The surface that declares a run's kind owns the run's words. It supplies
a **prompt file**: the system prompt as text with slots, and for each
kind the sentence that opens the request message.

| Slot | Becomes |
|---|---|
| `{kind}` | The kind's paragraph (its doc comment in the surface file) |
| `{declarations}` | The one `declare const clax`, holding every binding's operations, as the model reads it |
| `{surfaces}` | For each other binding: `# <name>` and its rules. Empty when there is none. The system prompt's last line |
| `{rules}` | The owning surface's own rules, then those of each of its namespaces the run has an operation of and the file does not place by name |
| `{rules.<namespace>}` | The rules of one namespace directly on the owning surface, in runs that have one of its operations |
| `{core.code_mode}`, `{core.data}`, `{core.images}`, `{core.finish}` | The core's standard paragraphs, for a prompt that wants them as written. `{core.images}` is nothing in a run that can be shown no image |
| `{<surface>.<name>}` | A slot of the owning surface's own, supplied by code that ships with the surface; a registered host has none |

Whose a slot is shows in its name: one with no `.` is the format's,
`core.` and `rules.` are the core's, and any other is the owning
surface's under its own name, so no surface is named `core` or `rules`.
A prompt file must have `{declarations}`, `{surfaces}` and, in its
request section, `{request}`, each once, and a file that leaves any of
the surface's rules without a place is refused
(`crates/clax-bot/src/prompt/file.rs` has the whole format).

`clax`'s prompt file is today's `prompt::system_prompt`, word for word,
with `{kind}` where the request paragraph is, `{declarations}` where the
declaration is and `{surfaces}` at the end. Its request openers are
today's two sentences, and its requests have the shape of the JSON
`prompt::request_message` writes (`clax.d.ts`). So a clax.dev run's
system prompt, declaration and request message are the bytes they are
now, and golden tests pin all three for both kinds, with and without web
tools.

A surface with no prompt file gets the core's standard one, made of the
`{core.…}` paragraphs and `{rules}`. `surface check` warns of a prompt file that has
neither `{core.data}` nor words of its own about what is data.

A run records each piece's name and hash: the prompt file, the kind,
each binding's rules and declaration.

## Registration and authority

A surface is **registered**: a definition, an optional prompt file and
settings, given by someone. Registering is open at run time; what a
registration may do follows from who made it.

| Registered by | How | Its runs act as | It may join other runs | Its rules and prompt are |
|---|---|---|---|---|
| The operator | Configuration, read when the daemon starts: a registration that is always there | The token's person | Wherever its joins say | The operator's words |
| A process with the daemon's token (ClaxShell's host) | `PUT /api/bot/surfaces/<name>` at run time | The owner | On the owner's artifacts, by its joins | The owner's agents' words, as everything they do is |
| A page, for its own viewers (a later project) | From the page, once the artifact declares it and whoever answers for the artifact approves, as for any capability that reaches past the page | The viewer, with no more than that viewer may do on that artifact, and a spending cap per app | None | Its own runs' only |
| A product's tab on a hosted Clax (a later project) | As the product's account | The product's end user | None | Its own runs' only |

- **Configuration is a convenience.** It is a registration the daemon
  makes for itself at start, so a host that wants one file and no code
  has that, and a host that wants to register when it connects, or
  change its definition without a restart, has that too.
- **A run records who started it,** and that starter's rights bound the
  run as its person's do. This is `Run::agent_grant`
  (`crates/clax-store/src/bot_runs.rs`), which bounds a run by the grant
  of the agent that asked for it, made general.
- **Words reach a prompt only with authority over the run.** A
  registration's prompt file governs runs of its own kinds, which act as
  its registrant. Its rules reach another kind's run only through a
  join, and a join needs the operator, the owner, or an approval.
- **A run is served by the registration that bound it.** Many
  registrations of one name may be live at once (a tab each, a page
  each); each answers its own runs' calls. On a daemon with one host
  there is one.
- **A runtime registration is a lease.** It lasts while its registrant
  is connected, and a short while after. Runs in flight keep the
  definition they were made with, by hash; with no one to answer, their
  calls to it are `unavailable`.

Pages running ClaxBot themselves are not in this design. They are why
registration is not tied to configuration, why serving is per
registration, and why an artifact's declaration and its approval
(`../claxos/docs/claxbot-services.md` §6) are the rule for a page: the
person who answers for an artifact has not chosen what its page's
script will tell a bot to do.

## A host's surface on a daemon

Configuration, as any `[bot]` setting (file or environment, read when
the daemon starts):

```toml
[bot.surfaces.shell]
definition = "/usr/share/claxshell/shell.d.ts"    # or a manifest, .json
prompt = "/usr/share/claxshell/shell.prompt.md"   # optional
allow = ["*"]                # operations offered at all
max_effects_per_run = 5      # every limit here may be "none"
call_timeout_s = 30
wait_limit = "24h"

[bot.surfaces.shell.runs]    # limits for runs of this surface's kinds;
max_steps = 40               # each defaults to the instance's
max_run_minutes = 30
```

A definition that does not parse is refused: from configuration,
`clax doctor` and `GET /api/bot/surfaces` say why; from a registration,
the answer does.

The protocol, with the daemon's token (the owner's agents' token, which
ClaxShell's host holds):

| Route | For |
|---|---|
| `PUT /api/bot/surfaces/<name>` | Registers a surface: a definition in either notation, an optional prompt file, settings within the instance's. Answers the registration's ID |
| `POST /api/bot/surfaces/<name>/serve` | Long-poll: delivers answers, collects calls |
| `POST /api/bot/surfaces/<name>/runs` | Starts a run of one of the surface's kinds, as the token's person |
| `POST /api/bot/surfaces/<name>/runs/<id>/messages` | Sends a message to one of its runs |
| `PUT /api/bot/surfaces/<name>/joins` | Says which scopes of which surface this one joins |
| `PUT /api/bot/surfaces/<name>/content` | Uploads bytes for an answer or a request |
| `GET /api/bot/surfaces/<name>/content/<id>` | Reads content the surface was passed |

```
POST /api/bot/surfaces/shell/serve
{ "wait_ms": 12000,
  "answers": [ {"id": "…", "result": …} | {"id": "…", "error": {"code": "…", "message": "…"}} ] }
→
{ "calls": [
  {"id": "…", "call": "write", "run": "…", "op": "settings.set", "args": […],
   "key": "…", "expires_in_ms": 29000},
  {"id": "…", "call": "read", "run": "…", "op": "ask", "args": […],
   "key": "…", "expires_in_ms": 86400000},
  {"id": "…", "call": "ended", "run": "…", "end": {"finished": {}}, "result": {…}} ] }

POST /api/bot/surfaces/shell/runs
{ "kind": "triage", "request": {…}, "bind": {"clax": {"artifact": "…"}}, "allow": ["…"] }
→ { "run": "…" }

PUT /api/bot/surfaces/shell/joins
{ "clax": [ {"artifact": "…"}, {"artifact": "…"} ] }
```

- `serve` returns as soon as there is a call, or after `wait_ms` (12 s
  at most). A call not answered goes out again on the next `serve` until
  its deadline. An answer is matched to its call by ID; one for an
  unknown or expired ID is ignored.
- A registration is **connected** while a `serve` is held or one ended
  within 15 seconds.
- Joins are kept by the daemon until replaced, so they hold while the
  host is away.
- **Progress.** `serve` also delivers events, which need no answer:
  `{"call": "event", "run": "…", "event": {…}}`. For a run a
  registration started, they are what its starter may see of the run:
  each step beginning and ending, each call and its outcome, parking and
  resuming, the model's own account of what it is doing. For a run it
  joined, they are the run's state (started, working, waiting on what,
  ended) and its own operations' calls, since the rest is another
  surface's and the run's person's.
- The messages are the protocol; HTTP with the daemon's token is how a
  local host carries them. A page or a hosted tab would carry the same
  messages another way.

`docs/contract.md` gains "ClaxBot surfaces": both notations, the
configuration and this protocol, written for a host to implement from,
with a contract fixture both sides replay
(as `../changelist/plugins/changeling/testdata/contract.json` does).

The hosted Worker takes no registrations yet; its runs have the
built-in surface alone.

## Tooling for hosts

| Command | Does |
|---|---|
| `clax bot surface check <file>` | Parses; prints what the model would read, or what is wrong: every entry's first mistake of a manifest (64 at the most), the first mistake of a `.d.ts` file |
| `clax bot surface convert <file>` | Writes a definition in the other notation |
| `clax bot surface gen --rust\|--ts <file>` | Emits a trait or interface with one method per operation and kind, and the types, so a host that drifts from its file does not compile |
| `clax bot surface test <file> --against <url>` | Drives a running host through the conformance suite: repeated keys, late answers, reconnects, content |
| `clax bot surface diff <old> <new>` | Says whether runs pinned to the old file can be served by a host on the new one |

## What the two example files showed

Writing ClaxShell's whole table and Changeling's tools and tasks in the
format changed the design in these places:

| Found while writing | Change |
|---|---|
| `review.d.ts` needed `Named` and `Listed` on every result | Interfaces may `extend` |
| Fields need a word of explanation for the model | Doc comments on fields are printed |
| Changeling marks third parties' text field by field; a request's one instruction was unmarked | `Instruction` marks the person's words; every other string is data. `Untrusted` is gone |
| Changeling's diff comes in 512 KiB parts, its pages up to 768 KiB | `diff` returns `Content`, read with `clax.content.text`; a call's result may be 1 MiB |
| Its categories depend on the subject's kind (`commits` needs `history` on a branch, `metadata` on a pull request) | The starter passes `allow` for the run; the provider refuses the rest with `forbidden` |
| The shell's parts change while it runs, and it wants `shell.*` on runs Clax starts | A standing join, set by the host beforehand |
| A drawn comment is made in ClaxShell's own UI | The host starts `shell.triage` itself; nothing claims a run |
| Fan-out is "open a thread there", which only `clax` can type | `clax.threads.open` and `clax.runs.wait`; no generic `spawn` |
| Each person's tab would serve its own runs of one surface | A run is served by the registration that bound it; many of one name may be live |
| A nested artifact needs the reviewer to be a person in Clax | Moved to "Not in this design" |

What it did not need: data a surface adds to a run it joins. A thread's
anchor is already `clax`'s request, and the theme is a read.

## The environment's policy

ClaxBot's limits already come from outside it: `RunSettings`, read
through `SettingsSource` (`crates/clax-bot/src/run.rs`). This design
finishes that, so that nothing about a run's size or shape is ClaxBot's
own decision.

```rust
trait RunPolicy {
    /// The limits for this run. Any of them may be absent.
    fn settings(&self, run: &RunInfo) -> Result<RunSettings>;
    /// What the model is sent for the next turn, from the run's
    /// persisted steps. `Overflow` asks for less than last time.
    fn history(&self, run: &RunInfo, steps: &[Step], why: HistoryFor) -> Result<History>;
    /// A call's deadline, when it parks, and how it is tried again.
    fn delivery(&self, run: &RunInfo, op: &OpInfo) -> Delivery;
    /// Whether a run in this state takes a message.
    fn accepts(&self, run: &RunInfo, state: RunState) -> Accept;
    /// What the model is shown of a value, given where it came from.
    fn to_model(&self, run: &RunInfo, value: Shown<'_>, journal: &Journal) -> Result<Value>;
}
enum HistoryFor { NextTurn, Overflow }
```

| Was | Becomes |
|---|---|
| `SettingsSource::settings()`, one set for the instance | `settings(run)`: per run, so a surface's kinds may have their own, within the instance's |
| `max_steps`, `max_run_minutes`, `max_run_credits` always set | Each may be absent. A run with none goes on until it finishes, is cancelled, or its person's credits run out |
| The limits this design adds (parked runs, children and depth, effects, deadlines, content sizes, surfaces a run) | Settings, with the defaults given here |
| History is every step, built in `run.rs` | `history`: the default policy is today's, every step. Another may send fewer, or a summary it keeps |
| A model that reports its context window full ends the run (`context_window`) | The policy is asked again with `Overflow`; the run gives up only if it answers the same or more |
| `MAX_TOOL_RESULT_BYTES` (`script.rs`), a constant | A setting |

What a history policy may not do: change the journal, the steps or the
audit. They are what happened; a history is only what the model is shown
of it, and the run records which it was shown (a piece, by hash, like
the prompt's). That is the Anvil review's warning, kept: a model's
working summary is separate from the record.

clax.dev's policy is today's values and the default history, so nothing
changes there.

## What stays the same on clax.dev

The requirement is that the way people use ClaxBot on clax.dev does not
change: sending a thread or a brief, what ClaxBot replies and when, the
draft and its publishing, follow-ups, credits. Internals may change, and
so may the detail of a run's own view ("What may change in a run's
view"). Steps 1 and 2 go further and change no byte a model
reads, because with no eval lane yet that is the cheapest proof that
behaviour is the same. Each of these is pinned by a test before the step
starts:

| What | Pinned by |
|---|---|
| The declaration, with and without web tools | `api.d.ts.golden`, `api.web.d.ts.golden`, unchanged (the spike reproduced both) |
| The system prompt, for `thread` and `brief`, with and without web | New goldens, taken from today's `prompt::system_prompt` |
| The request message | New goldens, taken from today's `prompt::request_message` |
| Which comments are requests | `clax`'s own words, unchanged: the sender's, marked `from_sender` |
| What a run may do, and how it ends | `host_conformance` and the run loop's tests, unchanged |
| Limits and history | clax.dev's policy: today's values |

### What may change in a run's view

The page that shows a run's steps is where this design's new facts
appear, and it may be updated for them:

| In the run view | May change because |
|---|---|
| A run's state | A parked run is neither working nor ended: it shows what it waits on |
| A call's error | New outcomes exist (`unavailable`, `expired`, `unknown_outcome`), and a retried call has attempts |
| How a script and its calls are shown | A script may be run again after a park; the view shows one step, with which calls were performed and which answered from the journal |
| Whose operations appear | A run may have a host's surface; a reader sees of it what its registration allows |
| What is listed as shown to the model | The request, messages, and what scripts returned or logged ("One door to the model") |

None of this changes what happens on a thread.

A later change may alter what the model reads (the tags, the order of
the prompt) if it shows by other evidence, such as recorded cases run
before and after, that what a person sees is the same.

What a person will see later, each as its own change: runs that wait;
clips.

## What is no longer in the way of a general agent

A general-purpose agent is not a goal. These were barriers in ClaxBot or
in earlier drafts of this design, and are not now:

| Barrier | Removed by |
|---|---|
| The person could not steer a run once it started | A run's inbox, and `Instruction` as a waiting call's answer: a run can be a conversation |
| "You are ClaxBot" and Clax's rules in every prompt | The kind's surface owns the prompt |
| Runs capped in steps and minutes | Limits are the environment's, and may be absent |
| A full context window ended the run | The history policy |
| One artifact a run; a draft and threads assumed | The core knows no nouns |
| Children could not have children, or outlive their parent | Starting a run is an operation's meaning; depth and budget are policy |

What such an agent would still need built, none of it blocked: a
compacting history policy, a provider for its tools, a live view of its
runs, and memory across runs.

## What measuring the run loop changed

A reading of the run loop as it stands
(`docs/superpowers/plans/2026-11-11-claxbot-surfaces-step2.md`) found
eight places where this design was wrong or thin. Each correction stands
over what earlier sections say.

| Found | Correction |
|---|---|
| Today's stop notices are posted before the terminal commit, journaled under the open step with the driver's claim, and their text is kept in the step's payload. After the commit the claim is gone, so `ended` cannot post them | A provider has two hooks. `ending(cx, end)` runs inside the terminal step and returns what the step's payload keeps; `ended` runs after the commit, for the roster and follow-ups. `End::Stopped` carries the limit's value ("40 steps") |
| `clax`'s draft is state of a step, committed in the step's transaction; draft operations are executed again on replay; a replayed publish changes the draft; writes are `WritePlan`s. Three methods cannot carry that | Inside the process a provider is a **step session**: begin, call (with its replay entry), finish (returning what to commit). The wire protocol is one adapter of it. "No path a host cannot take" means no path in the run loop |
| The request context is read at the first turn, after any wait in the queue, so a comment added meanwhile is in the prompt today | A run stores its request as the kind's surface gave it (for `clax`, IDs), and that surface resolves it at the first turn. "Fixed when the run is made" applies to bindings, not to a request's contents |
| The goldens this design named leave out the tools a model is sent, which typed `finish` changes | The pins cover the whole request body: the tools, their descriptions and schemas, and a tool result |
| A script's tool result is built once, at commit, and stored. Filtering when history is built would change earlier messages and the cached prefix | `to_model` runs once, when a step commits. `history` chooses among stored messages and may add a summary; it does not rewrite them |
| A model that reports its context window full takes two paths today: a `give_up` in the turn's payload, and a `model_error` stop on the provider's HTTP error. The overflowing turn is already charged | `Overflow` is a new kind of next step on both paths, and the charge stands |
| Absent caps are a store and admin-page change as well as a type change, and with no cap and no tick budget a native model call has no total timeout | Step 2 makes the caps optional in the policy; the admin pages and a floor for the model call's own timeout are named work, not a side effect |
| `artifact_id`, `base_version` and `thread_ids_json` are NOT NULL, `request` and `status` have CHECKs, so both the optional artifact and the parked status rebuild the table | One rebuild, in step 2, admitting `parked` before it is used. No legacy column is dropped in step 2 |

And one for step 3: the sandbox's operation list is fixed when a sandbox
is built, for all runs. "The sandbox does not change" holds for `clax`
alone; a run with a host's surface needs its operations given per run.

## The parser spike

A throwaway parser, checker and printer for the format, 900 lines of
Rust with `std` alone and no `unsafe`, compiled with `rustc` for the
host and for `wasm32-unknown-unknown`. It is not in the repository.

| Question | Result |
|---|---|
| Does today's table fit the format? | Yes. `clax.d.ts` holds all 24 operations (22 and the two web ones) and 24 interfaces |
| Is anything lost? | No. Printed from the parsed file in today's notation, the declaration equals `api.web.d.ts.golden` byte for byte (160 lines) |
| Does a run's subset work? | Yes. With `web.*` left out, the same print equals `api.d.ts.golden` byte for byte (128 lines): the operations go, and so do the interfaces only they named |
| Does printing round-trip? | Yes, for all three files: printed, parsed back to an equal definition, printed the same |
| Do the two hosts' files parse and pass the checks? | Yes: `shell` 10 operations, one waiting, one kind; `review` 9 operations, four kinds |
| Are mistakes refused? | Thirteen malformed files, each refused with its reason: a mistyped tag, no access tag, two, a generic, a `type` alias, an undeclared type, `@spend` in a host's file, a reserved name, `Instruction` in a result, a required parameter after an optional one, `any`, a kind with two parameters, an optional method |
| Is it TypeScript? | All three files pass `tsc --strict` beside a four-line declaration of the core types |

What it changed:

- **`true` and `false` are types,** so a union of interfaces may be told
  apart by a boolean field as well as a string one.
- **A word that starts with `@` begins a tag,** so tags come after the
  prose, and prose that needs such a word puts it in backticks.
- **`//` comments are not part of a definition.** Only doc comments are
  read and printed.

Since the spike, the format has allowed a kind a second parameter, for
messages ("A run's inbox"); the spike refused one.

What it did not try: checking a call's values against the parsed types
(`api::check_args` does this today over the same shapes; `extends` and
unions of interfaces are new to it), matching `@allow` patterns, JSON
Schema output, and line numbers on the errors found after parsing.

## From the Anvil review

"Anvil vs ClaxBot" (artifact `t48vx91858af`, sections 05 to 07) compares
ClaxBot with Anvil, a coding agent whose one tool is a JavaScript
interpreter.

**Rules here, each from a defect it found in Anvil:**

| Anvil's defect | The rule here | Held by |
|---|---|---|
| `search.grep` skipped the path check `fs` used | One check path. Every operation, kind, result and content read goes through the same checks; a provider has no second way in | The conformance suite calls each route with another run's IDs |
| A failed script was run again and repeated its effects | A step runs again only after a crash or a park, by journal replay, and replay performs nothing twice | A test parks, resumes and crashes one step and counts the provider's effects |
| A value that would not serialise replaced the saved state with `{}` | A value that fails its check changes nothing | Tests on `finish` and on requests |
| An old model reply was taken as the answer to a new request | Answers match calls by ID, never by what the run is waiting for | The protocol; an expired ID is dropped |

**The run's lifecycle is a pure function,** after Anvil's
`update(model, msg)`: runnable, parked, due and ended, and what moves a
run between them (an answer, a deadline, a cancel, a restart), are one
function from state and event to state and effects, tested by table with
no store and no clock.

**Left room for, as later projects:**

| The review's lesson | Needs from this design | Has it |
|---|---|---|
| Rebuild the exact context a model was sent | Prompt pieces recorded by name and hash; calls marked performed or replayed | Yes |
| Outcome cases and an on-demand eval lane | A typed result to check; providers' answers replayable from the journal | Yes |
| Bounded working memory | A built-in surface (`memory.get`, `memory.replace`) | Nothing more |
| An outside agent as a coding worker | An operation that waits and returns `Content` | Yes |

**Left out,** as the review advises: a JavaScript heap kept between
scripts, a model editing its own history, and undo by compensating
writes.

## Steps

1. **The file format.** Parser, printers, checks; `clax.d.ts` has
   replaced `OPS`; `surface check`. Nothing a model reads changes: the goldens
   stand. The spike showed the format holds today's table; this step
   writes the parser for keeps, with value checks over it.
2. **A core without nouns.** Providers, bindings, kinds as functions,
   typed `finish`, `ended`, prompt files, `RunPolicy`; `clax` owns
   artifacts, threads, drafts and replies. Done when a run with one fake
   surface and no `clax` passes, and every clax.dev golden still stands.
3. **Waiting, and the host protocol.** Parking and replay, the inbox,
   `to_model`;
   registration (configuration and the route), the manifest notation
   and `convert`, the seven routes, joins, starting runs; the contract
   section and fixture; `surface gen`, `test`, `diff`. Done when a test
   process answers a fixture surface's calls, one of them a day later by
   an injected clock, and no Clax source names that surface.
4. **Content.** The store, handles, upload and read, images to the
   model, `content.*`, `clax.files.put`.
5. **Fan-out and questions.** `clax.threads.open`, `clax.runs.wait`,
   `clax.ask`.

## What each host has after this

| ClaxShell asked for (`claxbot-services.md`) | After |
|---|---|
| Operations: read, write, effect (§1) | Yes |
| Request data (§2) | Yes: a kind's typed request, and reads |
| Rules and kinds (§3) | Yes |
| Fan-out with parent links and waiting (§4) | Yes: `clax.threads.open`, `clax.runs.wait` |
| Questions; confirming its own effects (§5) | Yes |
| Live events (§5) | Yes: progress over `serve` |
| Confirming Clax's `draft.publish` (§5) | No |
| Registration, `unavailable` (§6) | Yes: at run time, or by configuration |
| The artifact declares a service and the owner approves (§6) | Not for a host the operator or owner runs; it is the rule kept for pages |
| `clax.shell.screenshot()` | Yes: `Image` |

| Changeling needs | After |
|---|---|
| Its tools as typed operations | Yes: `review.d.ts` |
| Jobs with no artifact; results as notes | Yes |
| Structured comments, a quest log | Yes: `annotate`, `plan`, `quest.*` |
| Diffs and pages larger than a call | Yes |
| A tab that answers; many people's tabs | No: hosted |
| A run that waits for a closed tab to return | No: its reads answer `unavailable` |
| A Clax artifact nested in a review; reseller accounts | No: accounts |

## Calls made here, for the owner to check

1. A definition has two notations, a `.d.ts` file for people and a
   JSON manifest for programs, equal and convertible. Revised after the
   owner asked for the JSON Schema case to be weighed ("Two notations");
   awaiting the owner's verdict. Cost: two readers to keep equal.
2. Every surface lives on the one `clax` object
   (`clax.shell.windows()`), so no host name can meet a JavaScript
   global. Decided by the owner. Its cost: a namespace Clax adds later
   may be one a host already uses, and that host must rename.
3. Registration is open at run time, and configuration is one way to
   make one. Decided by the owner. What a registration may do follows
   from who made it ("Registration and authority").
4. A host registered by the operator or the owner's agents joins runs
   by a standing rule, with no approval per artifact. A page that
   registers a surface will need the artifact's declaration and an
   approval, and may join nothing. Revised after the owner asked how
   pages running ClaxBot themselves fit; awaiting the owner's verdict.
5. A run has an inbox. A kind may take messages, and whether a run in a
   given state accepts one is the environment's policy. Revised after
   the owner asked why continuing a run should be ruled out; awaiting
   the owner's verdict. `clax`'s kinds take none, so clax.dev is as it
   is.
6. A script awaits a call however long it takes; a slow call parks the
   run and the script resumes by replay. Revised after the owner asked
   why a waiting call should end its script; awaiting the owner's
   verdict. Cost: a script's work up to the wait is done again.
7. Bytes get in one way: an upload, then a handle. Never inline.
8. The model is shown only what a script returns or logs, the run's
   request and messages, and Clax's own account of a step: images
   included. Everything shown passes one function, where a guardrail can
   refuse what came from a source it does not trust. Widened after the
   owner's comment; awaiting the owner's verdict.
9. ClaxBot holds no deadlines. How long a call may take and how long a
   run may park are the environment's policy, per run and per call; the
   figures in this document are the daemon's defaults. Revised after
   the owner's comment; awaiting the owner's verdict.
10. Each operation says what repeating it means (`@once` or not), the
    run loop retries with the same key as the policy says, and a
    provider keeps its own account (`retry_after`, `accepted`). A script
    sees an error only when the policy is spent. Revised after the
    owner's comment, with a table of what a durable future needs;
    awaiting the owner's verdict.
11. Spending from the caller's budget is the meaning of
    `clax.threads.open`, not a rule of runs. Another operation may start
    a run with a budget and a life of its own; ClaxBot fixes only the
    parent link. Narrowed after the owner's comment; awaiting the
    owner's verdict.
12. The way people use ClaxBot on clax.dev does not change: threads,
    briefs, replies, drafts, publishing, follow-ups, credits. Internals
    may, and so may the detail of a run's view (states, errors, how
    scripts and calls are shown). Steps 1 and 2 keep the model's inputs
    byte-identical because that is the cheapest proof. As the owner
    stated it, in two comments; awaiting the owner's verdict.
13. Each registration says what a reader may see of it, nothing by
    default, and a run shows a reader the least any of its bindings
    allows. A run with `clax` alone shows what it shows today. Revised
    after the owner's comment; awaiting the owner's verdict.
14. A registration gets progress over `serve`: everything its starter
    may see for runs it started, and the run's state and its own calls
    for runs it joined. Revised after the owner's comment; awaiting the
    owner's verdict.
15. A parked run keeps its threads for as long as it waits, so a
    follow-up on that thread queues behind it.
16. Every model backend carries images the same way; the OpenAI and
    Gemini translators are changed in the content step. Revised after
    the owner's comment; awaiting the owner's verdict. Cost: the content
    step is larger.
17. The kind's surface owns the prompt. A host that writes its own
    could leave out the rule that says what is data; `surface check`
    warns, and nothing refuses it. The operator is trusted with words,
    as with rules.
18. A waiting call's answer may be an `Instruction`. The provider says
    whose words they are, as it already does when it starts a run. It
    keeps that standing with the model only if the script shows it
    whole.
19. No limit is compiled in. An environment that sets none gets runs
    bounded only by credits and cancelling.

## Not yet tried

- The run loop's split from `clax` (`run.rs` is 2,483 lines) is
  unmeasured, and so is what replaying a parked step costs.
- Neither product's maintainers have read their example file.
- Checking values against the parsed types.
- The history seam: how much of `run.rs` assumes it sends every step.
- The Worker's blob size limit.

## Left to the builder

The parser's structure; how runs, bindings, joins, parked calls and
content are laid out in the store and the migrations' numbers; whether
`clax` keeps its run columns or a table of its own; the generated code's
shape; test layout.
