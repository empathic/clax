//! The seam between the run loop and whatever provides a surface
//! (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`,
//! "Providers" as "What measuring the run loop changed" corrects it).
//! A script's calls go through it: [`crate::script`] is the run loop's
//! part of a script step and performs no operation, and Clax's own
//! surface is one provider ([`crate::clax::ClaxProvider`]). So does a
//! run's end: [`crate::ending`] is the run loop's part of the terminal
//! step, and what a run shows of itself at work and leaves when it ends
//! is each surface's own. This module is the contract, and this
//! documentation says, behaviour by behaviour, what is carried by which
//! part of it.
//!
//! # The parts
//!
//! - [`RunInfo`] is what the core knows of a run: its ID, person, model,
//!   kind, request as stored, and [`Binding`]s. No artifact, thread or
//!   draft is in it; those are a binding's scope and the provider's own
//!   rows.
//! - A [`SurfaceProvider`] is what the run loop holds for one binding. It
//!   yields its surface's definition ([`SurfaceProvider::definition`]),
//!   opens a [`StepSession`] for a step that runs a script, and is told
//!   of the run's steps and its end: [`SurfaceProvider::working`] by a
//!   tick about to take a turn or run a script,
//!   [`SurfaceProvider::ending`] inside the terminal step,
//!   [`SurfaceProvider::ended`] after its commit. The provider of the
//!   surface that declared a run's kind resolves the run's request for
//!   its first turn ([`SurfaceProvider::request`]); a run's audit
//!   records are filed on what its bindings' providers name
//!   ([`SurfaceProvider::audit_subject`]); and each provider is told
//!   when a driver starts ([`SurfaceProvider::driver_started`]).
//! - [`crate::registry::Providers`] is the provider of each surface, by
//!   its name, and makes of a run's row what a tick holds: the
//!   [`RunInfo`] and the provider of each binding. A binding of a
//!   surface with none registered is served by the provider the
//!   registry's resolver answers for it ([`crate::registry::Resolver`]:
//!   a surface a host registered while the daemon runs), and else by
//!   [`crate::registry::Unavailable`]
//!   ([`SurfaceProvider::unavailable`]).
//! - A [`StepSession`] is one attempt of one step, for one binding: it
//!   holds whatever of the provider's state belongs to the step (for
//!   `clax`, the draft), performs the step's calls
//!   ([`StepSession::call`]), and says at the end what the step's commit
//!   must include ([`StepSession::finish`], [`StepParts`]). A session
//!   that is dropped without `finish` is an attempt that never
//!   committed.
//! - [`dispatch`] is the core's part of one call, once the call is known
//!   to be the one its place recorded: it finds the binding the call is
//!   for ([`route`]), checks the arguments against that surface's
//!   definition ([`check_args`]), refuses an operation the binding does
//!   not have, hands every other to the binding's session ([`perform`]),
//!   and journals what the provider left to it ([`Claim::record`]).
//! - [`merge_fields`] joins what several bindings add to one step's
//!   payload, and [`StepCx::notice`] gives each binding journal places
//!   of its own for what its `ending` writes.
//! - [`FakeSurfaceProvider`] is a provider for tests, and
//!   [`crate::provider_conformance`] the cases every provider passes.
//!
//! A provider outside the process, which is sent a read or a write and
//! told of a run's end, is one implementation of this
//! ([`crate::outside::OutsideProvider`]): a session with no state that
//! sends each call out with its key and answers with [`Answer::of`], so
//! the core journals what comes back, or with [`Answer::waiting`].
//!
//! # One script step, in order
//!
//! | # | Who | Does |
//! |---|---|---|
//! | 1 | Core | Tells each binding's provider the run is working ([`SurfaceProvider::working`], `Tick::working` in `ending.rs`). Begins the step (`Store::begin_step_on`): its number and attempt. Loads the step's journal (`Store::bot_calls`), by place |
//! | 2 | Core | Opens one session for each binding, the surface that declared the run's kind first and the others in the order they were bound (the order they are finished and joined in too, row 7): [`SurfaceProvider::begin`] with a [`StepCx`] (the run, the binding, the step and attempt, the committed steps so far, the driver's claim, the cancel token). An error fails the tick; the step stays open and is begun again |
//! | 3 | Core | For each call the script makes, at place `seq` from 0, one call at a time: when an earlier call diverged, answers `replay_diverged`; when the journal holds another call at the place (another operation, or other arguments), the same, and notes the divergence. Neither is journaled, and no session sees the call. A call past a step's last place for a script's calls ([`CallPlace::in_script`]) is refused [`no_place`], unjournaled. Both refusals are the core's and are told like any call's answer: in the step's `calls`, as a `call` event and as a `bot.call` record, with no target, and counted on the requests line |
//! | 4 | Core | [`dispatch`], in this order. **Routes** the operation to its binding ([`route`]): an operation no binding takes is answered `unknown_op` (`clax.<op> is not an operation`). **Checks the arguments** against the definition of that binding's surface ([`SurfaceProvider::definition`], [`check_args`]): `unknown_op` in the same words for a name the definition does not have, `invalid_args` for arguments that do not hold. **Then asks whether the binding has the operation** ([`Binding::has`]): one outside its set is answered by [`SurfaceProvider::undeclared`], or [`not_declared`], and no session sees it |
//! | 5 | Provider | Any other call goes to the binding's session: [`StepSession::call`] with its [`CallPlace`], its checked and raw arguments, and the journal entry of the place when there is one |
//! | 6 | Core | Journals the answer as it says ([`Answer::journal`]), unless the place was in the journal (then nothing is written, whatever the answer says). The core's own refusals of row 4 are journaled as an outcome. Tells the run's events and the step's `calls` what [`CallNote`] says of the call |
//! | 7 | Core | When the script has ended: [`StepSession::finish`] on each session, with [`StepEnd::Commit`], or [`StepEnd::Cancelled`] when the script was cancelled. The parts are joined ([`StepJoin`]): their `tool_result` fields join what the model is told of the script, their `payload` fields the step's payload ([`merge_fields`]), their `draft` rows the step's commit (`StepCommit::draft`), in the commit's transaction. Fields that clash ([`FieldClash`]) abandon the attempt, which is counted against the step as a commit that failed; in a cancelled script's step the clash is counted, that binding's fields are left out, and the stop is taken all the same |
//! | 8 | Core | Commits. A driver that knows it lost its claim, a sandbox that could not take the script and a crash drop the sessions instead: nothing of rows 7 and 8 happens, and the step is executed again from 1. A claim may be lost without the driver knowing it yet (it learns when it next renews): then row 7 happens, and it is the store that refuses the commit (`run_not_claimed`) |
//!
//! The order of row 4 is what `clax`'s runs always had, when its one
//! surface made the route trivial: the arguments are checked before
//! the run is asked whether it declared the operation, so a web
//! operation the run did not declare, called with arguments that do
//! not hold, answers `invalid_args` and not `web_disabled`. With
//! several surfaces the arguments can only be checked once the call is
//! routed, since the route says whose definition they are checked
//! against. A script reaches the core with an operation no binding
//! takes only when the sandbox was built with more operations than the
//! run's surfaces have; the wording for a name that is no operation is
//! `api::check_args`'s.
//!
//! A session never begins or commits a step, and never writes the step's
//! payload: it writes only what its own writes need (a journal entry in
//! a write's transaction, through [`Claim::entry`]).
//!
//! # Who journals a call
//!
//! Every call of a step is journaled once, at its place, by exactly one
//! side. On an attempt where the place is already in the journal neither
//! side writes: [`perform`] skips the core's write, and a provider that
//! writes through one of the store's journaled paths is answered from
//! the entry there (`Store::with_bot_journal`).
//!
//! | The provider answers with | Who journals, when | What the entry holds |
//! |---|---|---|
//! | [`Answer::of`] ([`Journal::Answer`]) | The core, after the call returns ([`Claim::record`]) | The value, or the failure as `{code, message, retryable?}` |
//! | [`Answer::outcome`], [`Answer::refused`] ([`Journal::Outcome`]) | The core, after the call returns | `null` for a value (the provider performs the operation again on replay and needs none), or the failure |
//! | [`Answer::written`] ([`Journal::Written`]) | The provider, before it answers: in the transaction of the write the call made, or with what it charged | The provider's own form |
//! | [`Answer::unwritten`] ([`Journal::Unwritten`]) | No one | Nothing: the place has no row |
//! | [`Answer::waiting`] ([`Journal::Waiting`]) | No one, now: whoever brings the call's answer later, or the core when the call is made again and answered | Nothing yet: the place has no row |
//!
//! An answer made with `written`, a value or a failure, says the place
//! has its row. A provider that answers so without having written one
//! is at fault: the call made again with its key is answered the same by any
//! operation that keeps the contract of a key, but the place has no row
//! to tell a diverged replay by, and none for the `bot.call` record of
//! a call no committed attempt reported. The provider cases fail such a
//! provider.
//!
//! [`Answer::unwritten`] is the one way a call is left unjournaled by
//! both sides, and takes a failure alone (a value cannot be unwritten):
//! a failure of a call whose row and effect are one
//! transaction of the provider's that could not be made (its entry
//! refused, or the transaction failed), so that nothing took effect and
//! nothing was charged. The script is answered the failure, and the
//! step executed again makes the call again, where a failure the core
//! journaled would be answered again from its row. `clax`'s web
//! operations answer so (`clax/web.rs`), as they did before this seam.
//! A provider that wrote and answers `of` loses nothing:
//! `Store::record_bot_call` keeps the entry that is there. `of`,
//! `outcome` and `refused` given a failure write the same row,
//! `{code, message, retryable?}`: they differ only in what they keep of
//! a value. The core's own refusals (an operation no binding takes,
//! arguments that fail the definition's check, an operation the binding
//! does not have) are journaled by the core as an outcome:
//! `{code, message}`. `replay_diverged` is never journaled.
//!
//! Whoever writes a row, the core reads five things of every row of a
//! step, a provider's own included: its `op` and the hash of its
//! arguments, to tell a diverged replay (`script.rs`, `StepHost::call`),
//! and its `op`, `ok`, `attempt` and `seq`, for the `bot.call` records
//! of calls no committed attempt reported (`Tick::unreported_calls`,
//! `run.rs`). A row's `result` is read only by the side that wrote it.
//!
//! [`recorded`] reads an entry the core wrote. An entry a provider wrote
//! is read by that provider.
//!
//! # Clax's script calls, behind a session
//!
//! What each operation of Clax's own surface does in a session of
//! [`crate::clax::ClaxProvider`], and where: the files named are under
//! `crates/clax-bot/src/clax/`. "First" is a call whose place is not in
//! the journal; "replayed" one whose place is. Every row is what the
//! run loop did itself before its calls went through this seam. That
//! code was kept for the library's tests until the two were shown to
//! leave the same, run for run; what they left is pinned in the
//! fixtures of `src/script_scenarios.rs` (`tests/scenarios/script.*`).
//!
//! | Operation | Begin | First | Replayed | Journaled | Finish |
//! |---|---|---|---|---|---|
//! | **Before any operation** (`session.rs`, `ClaxSession::call`) | | Divergence and the argument check are the core's (`script.rs`, [`dispatch`]). The call's target (`api::target`) is the session's, in [`CallNote::target`]. The run's rules (`rules.rs`: a thread outside the request, `artifacts.create` outside a brief or a second time) are the session's: it refuses with the target set | The same checks run again; a refusal is answered again, not read from the journal | A refusal: by the core, `Outcome` | |
//! | **The draft's operations**: `files.*`, `draft.check`, `draft.theirs` (`draft.rs`) | `provider.rs`: loads the draft, `Draft::load` at the base the last committed step that moved it left (`payload.base`), else the run's. The base and `created` are folded from [`StepCx::history`] (`tally.rs`) | Performed over the session's draft | **Performed again**, over a draft loaded the same way: the journal's value is not read (it holds none) | By the core, `Outcome`: `null`, or the failure | `payload.changed`: the paths written or deleted this step. `draft`: the changes, committed with the step, so an attempt that does not commit leaves the stored draft as it was |
//! | **`draft.rebase`** (`draft.rs`) | As above | Reads the artifact's current version from the store, rebases the draft onto it | **Rebases onto the version the entry holds** (`result.version`), not the artifact's current one, so a version published since the first attempt changes nothing | By the core, `Answer`: the whole `Rebased` value. **A rebase that failed is journaled as its failure, which has no `version`**: run again it is not answered from the entry but performed again, onto the artifact's current version, and may answer otherwise | `payload.base`: the version the draft is now based on, when a rebase or a publish moved it, else `null` |
//! | **`draft.publish`** (`publish.rs`) | As above | Refuses a second publish in one script, `already_published`. Else `BotHost::plan_publish` with the whole draft, its base as `if_version`, and the run's threads as `addresses` when the call names none; then the plan runs with the place's entry ([`Claim::entry`]). Sets the draft's base to the published version | The host is not called. A recorded success **changes the session's draft again**: its base becomes the recorded version, and the session has published. A recorded failure is answered and changes nothing | A success: by the provider, `Written`, in the publish's transaction (`Store::publish_version_journaled`). A refusal or failure, the plan's or the write's: by the core, `Outcome`. **A refused publish is journaled**, so the next attempt is refused the same without asking the host | `payload.published` (`{version, url?}` or `null`), `payload.base`, and `tool_result.published` when the script published: the model is told the version |
//! | **A read of the host's** (`replies.rs`) | | `BotHost::read` | Answered from the entry ([`recorded`]); the host is not called | By the core, `Answer`: the value or the failure | |
//! | **A write of the host's**: `threads.reply`, `threads.resolve`, `db.set`, `db.update`, `db.delete`, `artifacts.create` (`replies.rs`) | | `BotHost::plan_write`, then the plan with the place's entry | Answered from the entry; the host is not called. **The session still counts it**: a successful `threads.reply` adds its thread to `replied` and a successful `artifacts.create` sets `created`, first or replayed | A success: by the provider, `Written`, in the write's transaction (`host.rs`, "The call journal"). A refusal or failure: by the core, `Outcome`. **A refused write is journaled** | `payload.replied`, `payload.created` |
//! | **`web.search`, `web.fetch`** (`web.rs`) | | Checks 3 to 9 of `web.rs`, then one transaction (`Store::spend_journaled`) holds the entry, the `bot.web` record, **the usage row charged and the page a fetch received**. [`Answer::charged`] and [`Answer::showing`] (`{"web": …}`) say what the run view shows | Answered from the entry (`from_entry`), before any other check: its `value`, its `cost_milli`, its `web`. Nothing is charged or counted again | By the provider, `Written`, for every call past check 2 that is journaled at all: a success, a refusal, a failure. **A call whose entry cannot be made ([`Claim::entry`]) or whose transaction fails (`Store::spend_journaled`) journals nothing, charges nothing and shows no cost**: the script is answered the failure ([`Answer::unwritten`], so that the core journals nothing either), and the step run again makes the call again. The entry's form is the provider's (`entry_result`: `value` or `code` and `message`, with `cost_milli`, `web`, `counted`, `source`, `format`), and the core never reads its `result` | Nothing: what a web call charged is already committed |
//! | **A web operation the run did not declare** (the first half of check 2: the binding lacks the operation) | | Not the session's: [`dispatch`] asks [`SurfaceProvider::undeclared`] (`web.rs`, `disabled`), which refuses `web_disabled` (`clax.<op> is not available to this run`), charged 0, showing `web`. Uncharged, uncounted, and with no `bot.web` record | The same answer, made again by `undeclared`, which is what the entry holds (a run's operations do not change, so the two cannot differ) | By the core, an outcome: `{code, message}` (see below) | |
//! | **A web operation the run declared, on a host with no web source** (the second half of check 2: the provider has no source) | | The session's: the binding has the operation, so the call reaches [`StepSession::call`], which refuses it before check 3 in the same words as the row above, charged 0, showing `web`, uncharged, uncounted, with no `bot.web` record | **Answered from the entry, as every web call given one** (before check 2): `from_entry` reads the core's `{code, message}` row, and a row written before this seam that also holds the two keys, to the same answer (it defaults the two). So a step first attempted on a host with no web source and run again on a host that has one is answered `web_disabled` again and does not reach the source | By the core, an outcome: the session answers [`Answer::refused`], and the row is `{code, message}`, the same as the row above | |
//!
//! The journal row of the two `web_disabled` refusals of check 2 is the
//! core's `{code, message}`. Before this seam the row also held
//! `cost_milli: 0` and `web` (the web performer's `entry_result`), and
//! the two keys are dropped. Nothing reads either
//! key from a refused call's row but `from_entry`, which defaults to
//! the same two values, so a row of either form answers the same, under
//! this code and the old: a run journaled by either and resumed by the
//! other left what one alone leaves, while the old code was kept to
//! drive it.
//! While the journal entry of such a call can be made, what the script
//! is thrown, the step's `calls` entry (`cost_milli: 0` and `web`, from
//! the answer's note) and the events did not change.
//! `tests/golden/journal.web-disabled.json` pins the row.
//!
//! **When the entry cannot be made, the answer itself differs**, and
//! this is the second thing a person can observe that the seam
//! changed. The code before the seam made the journal entry before it
//! looked at whether the refusal was check 2's, and answered the
//! entry's failure; the seam refuses first, without the store, as every
//! other refusal of the core's (arguments that do not hold) was always
//! made, and only then fails to journal it. Two cases:
//!
//! - **The driver no longer holds the run** (another took it): the
//!   script is thrown `web_disabled`, and the call's live event shows
//!   `{"code": "web_disabled", "cost_milli": 0}`, where before it was
//!   thrown `run_not_claimed` and the event showed that code and no
//!   cost. Someone watching the run's live events sees it until the
//!   next attempt's events replace the step's; nothing is journaled or
//!   committed on either code, and the tick waits as not its own
//!   (`web::a_web_operation_a_run_lacks_is_refused_as_itself_when_the_claim_is_lost`).
//! - **The entry cannot be made and the driver holds the run when the
//!   script ends**: the step goes on and commits, with `web_disabled`
//!   and `cost_milli: 0` in its `calls`, and the model is told the
//!   refusal's own words, where before the step committed the entry's
//!   failure and the model was told it (`run_not_claimed`, with the
//!   driver's ID). The refusal has no row, so a later attempt of the
//!   step makes it again. Pinned for a driver that lost the run only
//!   for the length of the call
//!   (`web::a_web_operation_a_run_lacks_is_refused_as_itself_in_a_step_that_commits`).
//!   What is not driven is the other cause: a read of the run's
//!   row failing in `Store::call_entry` though the claim was never
//!   lost, which the tests' store cannot be made to do.
//!
//! What a cancelled script keeps: the step becomes a `stopped` step
//! whose payload holds `published`, `replied` and `base`, without
//! `changed` or `created`, and no draft rows are committed; what the
//! script published and wrote stands. A session is told with
//! [`StepEnd::Cancelled`] and answers those fields alone, with no rows
//! (`session.rs`, `finish`). A script is cancelled when its error is
//! `Cancelled` **or the cancel token is set when it returns**, so a
//! script that returned a value may still end as `StepEnd::Cancelled`.
//!
//! What is the core's in a script step (`script.rs`): the step's
//! `tool_result` (`tool_result_content`) but for the fields a session
//! adds; the payload's `tool_use_id`, `title`, `code`, `outcome`,
//! `result` and `calls`; the `bot.script` and `bot.call` records, a
//! call's from its name, its [`CallNote::target`] and its outcome;
//! compute and the per-step usage lines. **A cancelled script's
//! `stopped` step is charged the script's compute and the per-step
//! lines counted with no call** (`Tick::end`, `ending.rs`), however
//! many calls the script made.
//!
//! ## What the rows above do not show
//!
//! - **The tool result is trimmed after the sessions' fields are
//!   joined.** `tool_result_content` cuts the logs, and then the text,
//!   to `RunLimits::tool_result_bytes`, with the sessions' fields
//!   (`published`) and `replay` already in it.
//! - **There are two lists of a script's calls.** The model is told the
//!   sandbox's (`outcome.calls`), which holds calls that never reached
//!   the core: one whose arguments the sandbox refused
//!   (`invalid_args`), and one made after the script ended
//!   (`script_finished`). The call over `max_calls` is in neither
//!   list: it ends the script, and the sandbox does not count it
//!   (`crates/clax-bot-sandbox/src/lib.rs`, "Host calls", "Limits";
//!   `tests/golden/script.error.call-limit.json`). The
//!   `bot.script` record lists the same operations. The payload's
//!   `calls`, the `bot.call` records and the count the infra usage line
//!   is charged for are of the core's own log: the calls that reached
//!   it, each with its [`CallNote`].
//! - **After a divergence no session sees a later call** (each is
//!   answered `replay_diverged` before anything is performed), the
//!   script's outcome becomes that error, and the step still commits
//!   what its sessions hold. The journal rows the earlier attempt left
//!   at and past the place that diverged stay: a publish among them
//!   took effect and is in no session's parts: the step commits
//!   `published: null` and no `base`, though the artifact has the
//!   version, and the next script's draft is loaded at the old base. A
//!   publish whose call was dropped by a cancel after its transaction
//!   committed leaves the same. The rows of places the committing
//!   attempt did not reach get a `bot.call` record each
//!   (`unreported_calls`).
//! - **A stop decided after a turn or a script was begun begins the same
//!   step again, as `stopped`** (`Tick::end`, `ending.rs`), and beginning
//!   an open step again raises its attempt. So
//!   [`SurfaceProvider::ending`] is given an attempt one higher than
//!   the session of that step had, with the same step number.
//!
//! # A run's end, through the two hooks
//!
//! The run loop decides that a run ends and how ([`End`]): `Tick::step`,
//! `must_stop` and the turn in `run.rs`, a cancelled script in
//! `script.rs`. `Tick::end` (`ending.rs`) then takes the terminal step:
//! it begins the step, asks each binding's `ending`, joins what they
//! answer to the step's payload ([`StepJoin`]), commits, and tells each
//! binding `ended`. The rows below say what `clax`'s provider does in
//! the two hooks (`clax/ending.rs`: `run_ending`, `notice`,
//! `what_i_did`, `post`, `run_ended`) and what stays the run loop's.
//! Every row is what the run loop did itself before a run's end went
//! through this seam, but for the two differences the rows name. That
//! code was kept for the library's tests until the two were shown to
//! leave the same, run for run and at each point a driver may die;
//! what they left is pinned in the fixtures of
//! `src/ending_scenarios.rs` (`tests/scenarios/ending.*`).
//!
//! | The run | The run loop (`ending.rs`, `run.rs`) | `clax`'s `ending` (`clax/ending.rs`) | What is journaled, and where | In the terminal step's payload |
//! |---|---|---|---|---|
//! | **Finished** | [`End::Finished`], with the kind's result (`{summary}`) for the surface that declared the kind | Posts the summary on every thread of the run when no thread was replied on since a script last changed the draft or published (`clax::Tally::replied`, folded from [`StepCx::history`]) and the summary is not empty; the web sources (`Store::web_pages`) follow the summary, or stand alone when the run replied itself | The provider's writes, each a `WritePlan` run with [`Claim::entry`] at the binding's notice places ([`StepCx::notice`]): the summary on thread `i` at place `i` (`NOTICE_SEQ + i`); a list that stands alone at `1 + i`. In the write's transaction, before the step commits | The hook answers `posted` and `sources`. **`posted` is true whenever the rule held**, whether or not a post succeeded. `summary` is the core's |
//! | **Gave up** | [`End::GaveUp`] | The notice `I gave up: <reason> <what I did>` on every thread | At place `i` | `notice`. `code` and `reason` are the core's |
//! | **Stopped**, for each reason of `must_stop`, the turn's own and a refusal | [`End::Stopped`]: the reason, the limit reached for `run_cap` (`40 steps`, `200 credits`, `30 minutes`, from the run's policy's limits), and when the person's credits reset for `credits_exhausted` | The words (`lead`, `notice`) and what the run did (`what_i_did`: the scripts, changed paths and published version in [`StepCx::history`]). **`cancelled` and `request_gone` post nothing**: the provider's choice, by the reason | The notice on every thread, at place `i` | `notice`: **the text whenever the stop has one**, whether or not it could be posted; `null` for `cancelled` and `request_gone`. `reason`, `error`, and what a cancelled script adds, are the core's. A stop also records a `bot.call` for each row of the step's journal under [`NOTICE_SEQ`] that no committed attempt reported (`Tick::unreported_calls`); a row at a notice place gets none |
//! | **Failed** | [`End::Failed`] | The notice `I stopped: I could not save my work. <what I did>`, or the lead of the code | At place `i` | `notice`. `reason` and `error` are the core's |
//! | **Notices come before the commit**: each is journaled under the open terminal step with the driver's claim. After the commit the claim no longer makes entries for the step, so `ended` cannot post them | `ending` is given the claim; `ended` is not | | A terminal step executed again (a crash before its commit) calls `ending` again: each write is answered from its entry and posts nothing more | The same fields on every attempt |
//! | **A notice that cannot be posted** is logged and the step commits | | `ending` answers `Ok` whatever its notices met, and `Err` only for what must keep the step open | | |
//! | **Sources that cannot be read** | **An `Err` is counted** ([`SurfaceProvider::ending`]): the `finish` is tried `max_commit_failures` times and the run then ends `failed` | The read of the sources is in `ending`, and its failure is the hook's one `Err`. Before the seam the read failed the tick uncounted, and the `finish` step was begun again for as long as it failed: this is the one behaviour the move changed, on purpose (`endings::sources_that_stay_unreadable_end_the_run_failed`) | | |
//! | **The roster, on** | `Tick::working`: every tick that is about to take a turn or run a script, before the step is begun. **Such a tick may still end the run**: no key, no time or credits for the turn, the request gone, a model error, a cancel | [`SurfaceProvider::working`]: `BotHost::working(run, true)`, which renews the roster record | Nothing | Nothing |
//! | **The roster, off, and follow-ups** | [`SurfaceProvider::ended`], after a commit that was made and not otherwise: a commit the store refused fails the tick, and a tick a test's crash hook stopped tells no one (the code before the seam turned the roster off from such a tick, before the commit with the run not ended; no driver that died did) | `BotHost::working(run, false)`, from which the server starts the threads' queued follow-ups (`crates/clax-server/src/bot.rs`) | Nothing: the run has ended | Nothing |
//!
//! `ending` is called on each attempt of the terminal step, for every
//! binding, the kind's surface first. An `ending` that fails, and
//! fields that clash, are counted against the step once for the
//! attempt, and the tick fails; in a `failed` step, and in the stop of
//! a cancelled script, the step commits without that binding's fields
//! (in the second, counted all the same).
//!
//! `ended` is called at least once, after the terminal commit, first by
//! the tick that committed. A crash between the commit and the call
//! loses that call, and it is made up for when a driver next starts
//! (`Driver::start`, `ending::redeliver`): each provider is asked which
//! runs it still awaits the end of ([`SurfaceProvider::awaiting_ended`]),
//! and told `ended` again for each of them that has ended. It is the
//! provider that remembers; the run loop keeps no record of who was
//! told. The repeat is rebuilt from what the store keeps of the run
//! (`ending::stored_end`), which is the kind of its end, its code and
//! its reason, and not every detail of the [`End`]
//! ([`SurfaceProvider::ended`]). `clax` awaits the end of the last run
//! of each thread that has a follow-up queued and no run at work, so a
//! run whose tick died is ended for it when that leaves someone
//! waiting; a roster record no `ended` turned off is one its own clock
//! drops. (After the repeats a starting driver tells each provider it
//! has started, [`SurfaceProvider::driver_started`], from which `clax`
//! starts the same follow-ups thread by thread,
//! `BotHost::start_waiting`.)
//!
//! The core's event for a committed step reads three of the fields a
//! provider returns: `step_summary` (`events.rs`) takes `changed` and
//! `published` from a script step's payload and `notice` from a
//! terminal step's. They stay `clax`'s names at the top of the payload
//! ([`merge_fields`]).
//!
//! # Several bindings in one step
//!
//! No run has two bindings yet, though the script step opens a session
//! for each binding it is given. What is fixed for when one does:
//!
//! - **Calls never overlap.** The sandbox sends a script's calls one at
//!   a time, and that holds across all of a step's sessions.
//! - **Payload fields.** A session's [`StepParts::payload`] and an
//!   `ending`'s fields are merged into the step's payload by
//!   [`merge_fields`]: `clax`'s under the provider's own names, at the
//!   top of the payload, which keeps today's bytes; any other
//!   surface's under one key, the surface's name. A name that is one of
//!   the core's own ([`PAYLOAD_KEYS`]), or that is taken already, is the
//!   provider's fault ([`FieldClash`]): the field is never dropped and
//!   never overwrites, the attempt is abandoned and counted against the
//!   step as a commit that failed is, and a step that reaches
//!   `max_commit_failures` ends the run `failed`. In that `failed`
//!   step itself there is nothing further to fall back to: a clash is
//!   logged, none of the clashing binding's fields are merged, the
//!   other bindings' fields are kept, and the step commits (as for an
//!   `ending` that fails there). The `stopped` step of a cancelled
//!   script is treated the same for its sessions' fields: the clash is
//!   logged and counted, that binding's fields are left out, and the
//!   stop is taken in the same tick, the script's compute charged. Which top-level names each surface
//!   made is kept beside the payload for the step ([`Merged`]), so a
//!   surface's own object is told from a field of the same name that
//!   another binding put there. The fields of
//!   [`StepParts::tool_result`] are joined the same way, over
//!   [`TOOL_RESULT_KEYS`]. A surface whose own name is one of those
//!   names, or one of `clax`'s field names ([`CLAX_FIELDS`]), would
//!   clash on every step it adds fields to: a host's surface so named
//!   is refused where it is defined (`clax_surface::STEP_FIELDS`).
//! - **Journal places for `ending`.** Each binding has its own range
//!   of [`NOTICE_STRIDE`] places for what its `ending` writes
//!   ([`StepCx::notice`]): the kind's surface from [`NOTICE_SEQ`], the
//!   run's other bindings each one stride higher, in the order they
//!   were bound. So two bindings neither take one place (`key_reused`)
//!   nor are answered from each other's entry. A run whose one binding
//!   is its kind's has exactly today's places.
//! - **`ending` for every binding.** One binding's error does not keep
//!   another's `ending` from being called in that attempt
//!   ([`SurfaceProvider::ending`]).
//!
//! # A call that waits
//!
//! A session has a third answer beside a value and a failure
//! ([`Answer::waiting`], the design's "Calls that wait"): the provider
//! holds the call under its key and has no answer for it yet. What it
//! promises by that answer is said on [`StepSession::call`].
//!
//! - **Only an operation whose definition says `@waits` may answer
//!   so.** From any other operation, and for a call that came with its
//!   entry, the core takes the answer for the provider's mistake and
//!   refuses the call `provider_error` ([`CODE_PROVIDER_ERROR`]),
//!   journaled as any refusal of the core's ([`perform`]). What
//!   [`SurfaceProvider::undeclared`] answers is a refusal whatever it
//!   says, and the core's own refusals are built here: neither waits.
//! - **No one journals it and the script is told nothing.** The place
//!   keeps no row ([`Journal::Waiting`]), and [`dispatch`] and
//!   [`perform`] hand the answer up to their caller as it is
//!   ([`Answer::is_waiting`]). What the script step does with it is the
//!   step's (`script.rs`, `StepHost::ask`): the script is not answered,
//!   its attempt is left as a crashed one is, with nothing committed
//!   or charged, and the run parks on the call until its answer is
//!   journaled or its deadline comes ([`Answer::waiting_until`]).
//! - **The answer comes later, by one of two ways.** Whoever brings it
//!   journals it at the call's place, in the row the core writes for
//!   [`Answer::of`], and the step run again makes the call with that
//!   entry; or the step is run again first, the call is made again
//!   under its key with no entry, and the provider answers it then, a
//!   value, a failure or waiting once more.
//!
//! A run waits on one call at a time: a script's calls are made one at
//! a time, and the call after one that waits is never made in that
//! attempt.

use crate::api::{CODE_BAD_ARGS, CODE_UNKNOWN_OP};
use crate::host::{NOTICE_SEQ, args_hash, host_error};
use clax_bot_sandbox::{CODE_WAITING, CancelToken, HostError, MaybeSendSync};
use clax_core::audit::{AuditIds, AuditRecord};
use clax_store::Store;
use clax_store::bot_pending::{EndFor, PendingCall};
use clax_store::bot_runs::{
    CLAX_SURFACE, CallEntry, DraftChange, DriverId, Run, RunStatus, Step, StepCommit, StepKind,
    StoredBinding,
};
use clax_store::credits::Metered;
use clax_surface::{Author, Definition, Limits, Reserved};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex};

/// Named fields of a step's payload, or of what a model is told of a
/// script.
pub type Fields = serde_json::Map<String, Value>;

// ---------------------------------------------------------------------------
// A run and its bindings
// ---------------------------------------------------------------------------

/// One surface a run has: fixed when the run is made.
///
/// The store keeps a binding as a [`StoredBinding`], whose version 1
/// holds `granted`, the operations a run was given beyond those every
/// run of the surface has, and not the run's operations.
/// [`Binding::from_stored`] makes this type of it with the surface's
/// file, so `operations` is the whole set and nothing downstream reads
/// `granted`.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    /// The surface's name: `clax` for Clax's own.
    pub surface: String,
    /// The SHA-256 of the surface's definition as the run was made with
    /// it, in lowercase hex. `None` for a binding made before definitions
    /// were kept by hash, which every stored binding of version 1 is.
    pub hash: Option<String>,
    /// The provider's own value for what the run is bound to (for `clax`,
    /// `{"artifact": "<artifact ID>"}`). The core does not read it.
    pub scope: Value,
    /// The operations the run has of the surface, in the file's order,
    /// each by the name a script calls after `clax.` and the journal
    /// and an allow list know it by (`Definition::operations`):
    /// `files.read` for one of `clax`, `shell.windows` for one of a
    /// host's surface `shell`. A call to any other is refused before a
    /// session sees it ([`dispatch`]).
    pub operations: Vec<String>,
}

/// One binding of a run as its row keeps it, for whoever chooses the
/// binding's provider ([`SurfaceProvider::for_run`],
/// [`crate::registry::Resolver`]): a stored binding of either version
/// ([`StoredBinding`]), owned.
#[derive(Clone, Debug, PartialEq)]
pub struct BindingAsStored {
    /// The surface's name.
    pub surface: String,
    /// The hash of the surface's definition as the run was made with
    /// it; `None` for a binding that keeps none.
    pub hash: Option<String>,
    /// The ID of the registration that serves the binding, for a
    /// surface a host registered; `None` for a binding that names none
    /// (`clax`'s).
    pub registration: Option<String>,
    /// The SHA-256 of the prompt file the run was started with, in
    /// lowercase hex ([`StoredBinding::prompt`]); `None` for a binding
    /// started with none, whose run is sent the standard prompt.
    pub prompt: Option<String>,
    /// The provider's own value for what the run is bound to.
    pub scope: Value,
    /// The operations the run was given beyond those every run of the
    /// surface has ([`StoredBinding::granted`]).
    pub granted: Vec<String>,
    /// The run's operations of the surface, each by the name a script
    /// calls it by after `clax.`, for a binding that keeps them;
    /// `None` for one that does not (the provider says which the run
    /// has: [`SurfaceProvider::standing`]).
    pub operations: Option<Vec<String>>,
}

impl BindingAsStored {
    /// `stored`, as the store read it: a binding of version 2 names
    /// its registration and holds the run's operations, and one of
    /// version 1 holds what the run was granted.
    #[must_use]
    pub fn of(stored: &StoredBinding) -> BindingAsStored {
        BindingAsStored {
            surface: stored.surface().to_string(),
            hash: stored.hash().map(str::to_string),
            registration: stored.registration().map(str::to_string),
            prompt: stored.prompt().map(str::to_string),
            scope: stored.scope().clone(),
            granted: stored.granted().to_vec(),
            operations: stored.operations().map(<[String]>::to_vec),
        }
    }
}

impl Binding {
    /// The binding `stored` is, for a surface whose file is `definition`
    /// (its provider's, [`SurfaceProvider::definition`]): each operation
    /// of the file that every run of the surface has
    /// (`standing` answers true for its name) or that `stored` grants,
    /// in the file's order. A granted name the file does not have gives
    /// the run nothing.
    ///
    /// `standing` is the surface's own rule, which a stored binding of
    /// version 1 does not hold: it keeps only what a run was granted
    /// beyond it ([`StoredBinding::granted`]). The surface's provider
    /// says it ([`SurfaceProvider::standing`]): for `clax`, every
    /// operation but `web.*`.
    #[must_use]
    pub fn from_stored(
        stored: &StoredBinding,
        definition: &Definition,
        standing: impl Fn(&str) -> bool,
    ) -> Binding {
        let granted = stored.granted();
        Binding {
            surface: stored.surface().to_string(),
            hash: stored.hash().map(str::to_string),
            scope: stored.scope().clone(),
            operations: definition
                .operations()
                .map(|o| o.name)
                .filter(|name| standing(name) || granted.iter().any(|g| g == name))
                .map(str::to_string)
                .collect(),
        }
    }

    /// The binding `stored` is, served by `provider`. A binding of
    /// version 2 holds the run's operations of the surface, fixed when
    /// the run was made, and they are the binding's as the row lists
    /// them: nothing is worked out again from the running build. A
    /// binding of version 1 has those the provider's definition and
    /// standing rule give it, and what it was granted
    /// ([`Binding::from_stored`]).
    #[must_use]
    pub fn as_stored(stored: &StoredBinding, provider: &dyn SurfaceProvider) -> Binding {
        match stored.operations() {
            None => Binding::from_stored(stored, provider.definition(), |op| provider.standing(op)),
            Some(listed) => Binding {
                surface: stored.surface().to_string(),
                hash: stored.hash().map(str::to_string),
                scope: stored.scope().clone(),
                operations: listed.to_vec(),
            },
        }
    }

    /// Whether the run has operation `op` (`files.read`,
    /// `shell.windows`) of the surface.
    #[must_use]
    pub fn has(&self, op: &str) -> bool {
        self.operations.iter().any(|o| o == op)
    }
}

/// What the core knows of a run, and tells a provider and a policy:
/// made once a tick from the run's row
/// ([`crate::registry::Providers::bind`]). It names no artifact, thread
/// or draft: a provider that needs its own rows of the run reads them
/// by [`RunInfo::id`], or keeps the row a tick hands it
/// ([`SurfaceProvider::for_run`]).
#[derive(Clone, Debug, PartialEq)]
pub struct RunInfo {
    /// The run's ID.
    pub id: String,
    /// The person key the run acts for.
    pub person: String,
    pub model: String,
    /// `<surface>.<kind>`: `clax.thread`, `clax.brief`.
    pub kind: String,
    /// The request as stored, which the kind's surface gave when the run
    /// was made (for `clax`, `{"thread_ids": […]}`) and resolves at the
    /// run's first turn.
    pub request: Value,
    /// The run's bindings, in the order they were bound.
    pub bindings: Vec<Binding>,
}

impl RunInfo {
    /// The run's binding of `surface`, when it has one.
    #[must_use]
    pub fn binding(&self, surface: &str) -> Option<&Binding> {
        self.bindings.iter().find(|b| b.surface == surface)
    }

    /// The surface that declared the run's kind: what precedes the first
    /// `.` of [`RunInfo::kind`].
    #[must_use]
    pub fn kind_surface(&self) -> &str {
        self.kind.split('.').next().unwrap_or("")
    }
}

/// The binding among `bindings` whose surface a script's call to `op`
/// is for (`op` as the script writes it after `clax.`): the binding of
/// the surface `op` begins with (`shell.windows` is `shell`'s), else
/// the `clax` binding, whose operations carry no surface name
/// (`files.read`). Whether that surface has such an operation is its
/// definition's to say ([`check_args`]), and whether the binding has it
/// [`Binding::has`]'s; [`dispatch`] asks both, in that order.
///
/// `None` when the run has neither binding: the operation is one no
/// surface of the run takes, and [`dispatch`] answers it
/// [`no_such_operation`].
#[must_use]
pub fn route<'a>(bindings: &'a [Binding], op: &str) -> Option<&'a Binding> {
    route_by(bindings, |b| b, op)
}

/// [`route`] over anything that holds a binding.
fn route_by<'a, T>(held: &'a [T], binding: impl Fn(&T) -> &Binding, op: &str) -> Option<&'a T> {
    let of = |surface: &str| held.iter().find(|h| binding(h).surface == surface);
    if let Some((surface, _)) = op.split_once('.')
        && surface != CLAX_SURFACE
        && let Some(found) = of(surface)
    {
        return Some(found);
    }
    of(CLAX_SURFACE)
}

// ---------------------------------------------------------------------------
// A step, a place in it, and the driver's claim
// ---------------------------------------------------------------------------

/// A call's place in a step: what [`crate::host::OpCtx`] holds of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallPlace {
    pub step: i64,
    /// The attempt of the step that makes the call.
    pub attempt: i64,
    /// The call's place among the step's calls, from 0. The places from
    /// [`NOTICE_SEQ`] are the terminal step's, for what
    /// [`SurfaceProvider::ending`] writes ([`StepCx::notice`]).
    pub seq: i64,
    /// `<run>:<step>:<seq>`: the same on every attempt of the step. A
    /// write made twice with one key takes effect once and answers the
    /// same.
    pub key: String,
}

impl CallPlace {
    /// Place `seq` of attempt `attempt` of step `step` of run `run_id`.
    #[must_use]
    pub fn new(run_id: &str, step: i64, attempt: i64, seq: i64) -> CallPlace {
        CallPlace {
            step,
            attempt,
            seq,
            key: format!("{run_id}:{step}:{seq}"),
        }
    }

    /// Place `seq` among a script's calls in attempt `attempt` of step
    /// `step` of run `run_id`: from 0, and under [`NOTICE_SEQ`], where
    /// the places for what the bindings' `ending`s write begin
    /// ([`StepCx::notice`]). `None` outside them: the core answers such
    /// a call [`no_place`] and neither journals nor performs it.
    ///
    /// The one place a script's call is given its place, whichever
    /// binding it is for: the run loop numbers a script's calls through
    /// it, and [`StepCx::place`] is this for a context's own step.
    #[must_use]
    pub fn in_script(run_id: &str, step: i64, attempt: i64, seq: i64) -> Option<CallPlace> {
        (0..NOTICE_SEQ)
            .contains(&seq)
            .then(|| CallPlace::new(run_id, step, attempt, seq))
    }
}

/// The driver's claim on a run, lent to a provider for one step: with
/// it a provider makes the journal entry of a place, to write in the
/// transaction of its own write. It is good only while the driver holds
/// the run.
#[derive(Clone, Copy)]
pub struct Claim<'a> {
    store: &'a Store,
    driver: &'a DriverId,
    run_id: &'a str,
}

impl std::fmt::Debug for Claim<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claim")
            .field("driver", &self.driver)
            .field("run_id", &self.run_id)
            .finish_non_exhaustive()
    }
}

impl<'a> Claim<'a> {
    /// `driver`'s claim on run `run_id` of `store`.
    #[must_use]
    pub fn new(store: &'a Store, driver: &'a DriverId, run_id: &'a str) -> Claim<'a> {
        Claim {
            store,
            driver,
            run_id,
        }
    }

    /// The driver that holds the run, for a store path that checks it
    /// (`Store::spend_journaled`).
    #[must_use]
    pub fn driver(&self) -> &'a DriverId {
        self.driver
    }

    /// The run the claim is on.
    #[must_use]
    pub fn run_id(&self) -> &'a str {
        self.run_id
    }

    /// The pending call the run's store keeps for `place`, waiting or
    /// not, when it keeps one: a call that was opened there for a
    /// surface outside the process (`Store::open_call`), by this
    /// attempt or an earlier one. `None` also when it cannot be read.
    #[must_use]
    pub fn pending_call(&self, place: &CallPlace) -> Option<PendingCall> {
        self.store
            .pending_call_at(self.run_id, place.step, place.seq)
            .ok()
            .flatten()
    }

    /// The journal entry of the call to `op` on `args` (the JSON array the
    /// script passed) at `place`, not yet answered: to run a
    /// [`crate::host::WritePlan`] with, or to answer and write with what
    /// the call charged.
    ///
    /// # Errors
    /// `run_not_claimed` when the driver no longer holds the run; the
    /// store's, as a host error.
    pub fn entry(&self, place: &CallPlace, op: &str, args: &Value) -> Result<CallEntry, HostError> {
        self.store
            .call_entry(
                self.run_id,
                self.driver,
                place.step,
                place.attempt,
                place.seq,
                op,
                &args_hash(args),
            )
            .map_err(|e| host_error(&e))
    }

    /// The core's part of journaling: writes `answer` to the call to `op`
    /// on `args` at `place` as [`Answer::journal`] says, and nothing for
    /// [`Journal::Written`], [`Journal::Unwritten`] and
    /// [`Journal::Waiting`]. For a call
    /// whose place the journal does not hold; an entry already there is kept as it is. An entry that
    /// cannot be made or written is logged: the script has its answer,
    /// and the step executed again makes the call again.
    pub fn record(&self, place: &CallPlace, op: &str, args: &Value, answer: &Answer) {
        debug_assert!(
            answer.journal() == Journal::Written || answer.note().cost_milli().unwrap_or(0) <= 0,
            "a call that charged is journaled by its provider, with its charge"
        );
        let keep = match answer.journal() {
            Journal::Written | Journal::Unwritten | Journal::Waiting => return,
            Journal::Answer => true,
            Journal::Outcome => false,
        };
        let Ok(entry) = self.entry(place, op, args) else {
            return;
        };
        let entry = entry.answered(
            answer.result().is_ok(),
            match answer.result() {
                Ok(v) if keep => v.clone(),
                Ok(_) => Value::Null,
                Err(e) => err_json(e),
            },
        );
        if let Err(e) = self.store.record_bot_call(&entry) {
            tracing::warn!(
                "clax-bot: journaling call {} of run {}: {e}",
                place.seq,
                self.run_id
            );
        }
    }
}

/// The answer an entry the core journaled holds ([`Answer::of`],
/// [`Answer::outcome`], [`Answer::refused`]): its value (`null` for an
/// outcome), or its failure with its `retryable` flag. An entry a
/// provider wrote itself ([`Answer::written`]) has that provider's form,
/// which this may not read.
///
/// # Errors
/// The failure the entry recorded.
pub fn recorded(entry: &CallEntry) -> Result<Value, HostError> {
    if entry.ok() {
        return Ok(entry.result().clone());
    }
    let r = entry.result();
    let failure = HostError::new(
        r["code"].as_str().unwrap_or("call_failed"),
        r["message"].as_str().unwrap_or(""),
    );
    Err(if r["retryable"] == json!(true) {
        failure.retryable()
    } else {
        failure
    })
}

/// A failure as a journal entry keeps it: `code`, `message`, and
/// `retryable` when it is. What [`recorded`] reads back.
pub(crate) fn err_json(e: &HostError) -> Value {
    let mut v = json!({"code": e.code, "message": e.message});
    if e.retryable {
        v["retryable"] = json!(true);
    }
    v
}

/// How many journal places each binding of a run has for what its
/// [`SurfaceProvider::ending`] writes: the kind's surface from
/// [`NOTICE_SEQ`], each other binding one stride higher than the one
/// before it.
pub const NOTICE_STRIDE: i64 = 1_000_000;

/// What a provider is given for one attempt of one step: to open a
/// session with, and inside the terminal step.
#[derive(Clone, Copy, Debug)]
pub struct StepCx<'a> {
    pub run: &'a RunInfo,
    /// The run's binding of the provider's surface.
    pub binding: &'a Binding,
    /// The step, from 1.
    pub step: i64,
    /// Which attempt of the step this is, from 1: one more each time the
    /// step is begun again uncommitted.
    pub attempt: i64,
    /// The run's committed steps, in order, each with its payload: what
    /// the provider's earlier sessions and hooks returned is among their
    /// fields, and a provider folds what it needs of the run so far
    /// from them (for `clax`: the draft's base, whether the run created
    /// an artifact, the threads replied on, the paths changed).
    pub history: &'a [Step],
    pub claim: Claim<'a>,
    /// Cancelled when the step is: a provider passes it to work that may
    /// take long.
    pub cancel: &'a CancelToken,
    /// The first of the binding's journal places for what `ending`
    /// writes.
    notices: i64,
}

impl<'a> StepCx<'a> {
    /// The context of attempt `attempt` of step `step` of `run`, for its
    /// binding `binding`. The binding's journal places for what `ending`
    /// writes ([`StepCx::notice`]) are worked out here: from
    /// [`NOTICE_SEQ`] for the binding of the surface that declared the
    /// run's kind, and for the run's `k`th other binding, in the order
    /// they were bound and from 1, from `NOTICE_SEQ + k * NOTICE_STRIDE`.
    /// A binding that is not one of the run's has the range after the
    /// last of them.
    #[must_use]
    pub fn new(
        run: &'a RunInfo,
        binding: &'a Binding,
        step: i64,
        attempt: i64,
        history: &'a [Step],
        claim: Claim<'a>,
        cancel: &'a CancelToken,
    ) -> StepCx<'a> {
        let kind = run.kind_surface();
        let others = || run.bindings.iter().filter(|b| b.surface != kind);
        let k = if binding.surface == kind {
            0
        } else {
            others()
                .position(|b| b.surface == binding.surface)
                .unwrap_or_else(|| others().count())
                + 1
        };
        StepCx {
            run,
            binding,
            step,
            attempt,
            history,
            claim,
            cancel,
            notices: NOTICE_SEQ
                + i64::try_from(k).unwrap_or(i64::MAX / NOTICE_STRIDE - 1) * NOTICE_STRIDE,
        }
    }

    /// Place `seq` of this attempt, among a script's calls: from 0, and
    /// under [`NOTICE_SEQ`], where the places for what the bindings'
    /// `ending`s write begin ([`StepCx::notice`]). `None` outside them:
    /// the core answers such a call [`no_place`] and neither journals
    /// nor performs it.
    ///
    /// A script's calls are bounded by `RunSettings::script_max_calls`,
    /// and the setting behind it (`script.max_calls`) accepts values far
    /// above [`NOTICE_SEQ`], so the sandbox's limit does not keep a
    /// script under it. This is the backstop for that ([`no_place`]).
    #[must_use]
    pub fn place(&self, seq: i64) -> Option<CallPlace> {
        CallPlace::in_script(&self.run.id, self.step, self.attempt, seq)
    }

    /// The binding's `i`th journal place, from 0, for what
    /// [`SurfaceProvider::ending`] writes in this attempt of the
    /// terminal step. No other binding of the run is given it, so a
    /// write there can neither meet another binding's key nor be
    /// answered from another binding's entry. `None` outside the
    /// binding's [`NOTICE_STRIDE`] places.
    #[must_use]
    pub fn notice(&self, i: i64) -> Option<CallPlace> {
        (0..NOTICE_STRIDE)
            .contains(&i)
            .then(|| CallPlace::new(&self.run.id, self.step, self.attempt, self.notices + i))
    }
}

// ---------------------------------------------------------------------------
// A call and its answer
// ---------------------------------------------------------------------------

/// One call of a script, as a session is given it.
#[derive(Clone, Copy, Debug)]
pub struct Call<'a> {
    pub place: &'a CallPlace,
    /// The operation, as the script names it after `clax.` and as the
    /// journal, the run's events and the binding name it
    /// ([`Binding::operations`]).
    pub op: &'a str,
    /// The arguments, checked against the surface's file: one value for
    /// each parameter, `null` for an optional one left out.
    pub args: &'a [Value],
    /// The arguments as the script passed them, a JSON array: what the
    /// journal's hash is of ([`Claim::entry`]).
    pub raw: &'a Value,
    /// The journal entry of this place, when an earlier attempt of the
    /// step left one: the same operation on the same arguments (the core
    /// has checked). A provider answers from it, performs the operation
    /// again, or both, as the operation needs; it performs no write or
    /// charge that the first attempt made.
    pub replay: Option<&'a CallEntry>,
}

/// Who journals a call whose place the journal does not hold (see the
/// module documentation, "Who journals a call"). A provider says which
/// by the constructor it builds its [`Answer`] with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Journal {
    /// The provider has: in the transaction of the write the call made,
    /// or with what it charged. The core writes nothing.
    Written,
    /// The core does, with the answer: the value, or the failure.
    Answer,
    /// The core does, without the value: that the call succeeded, or the
    /// failure. For an operation the provider performs again on replay.
    Outcome,
    /// No one does: the call failed and left nothing written, no row
    /// and no effect ([`Answer::unwritten`]). The place stays out of the
    /// journal, and the step executed again makes the call again.
    Unwritten,
    /// No one does now: the call has no answer yet
    /// ([`Answer::waiting`]). The place stays out of the journal until
    /// the answer is brought to it, or the call is made again and
    /// answered.
    Waiting,
}

/// The host error code of a call whose provider answered what it may
/// not: that the call waits, from an operation that is not tagged
/// `@waits` or for a call that came with its entry ([`perform`]).
pub const CODE_PROVIDER_ERROR: &str = "provider_error";

/// The names the core gives a call in the run's events and the step's
/// `calls`, which no field of [`CallNote::extra`] takes.
pub const CALL_KEYS: [&str; 6] = ["code", "cost_milli", "ms", "ok", "op", "target"];

/// What a provider says of a call beside its answer, for the run's
/// events, the step's `calls` and the call's audit record. Built with
/// an [`Answer`] ([`Answer::naming`], [`Answer::charged`],
/// [`Answer::showing`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CallNote {
    target: Option<String>,
    cost_milli: Option<i64>,
    extra: Fields,
}

impl CallNote {
    /// What the call names: a path, a thread, a document, the artifact.
    #[must_use]
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// What the call charged, in millicredits, for an operation that is
    /// charged.
    #[must_use]
    pub fn cost_milli(&self) -> Option<i64> {
        self.cost_milli
    }

    /// The provider's own fields of the call, which the run view shows
    /// beside the core's: for a web call of `clax`, `web`, its `{query}`
    /// or `{url, source}`. The field named `web` is the one the core
    /// puts in a `call` event's `web` (`Event::Call`, which has that
    /// one field for a provider's today); every field goes into the
    /// step's `calls` ([`CallNote::add_to`]).
    #[must_use]
    pub fn extra(&self) -> &Fields {
        &self.extra
    }

    /// Adds what the note says beside the target to `call`, a call's
    /// JSON object in the run's events or a step's `calls`:
    /// `cost_milli` when the call was charged, and each field of
    /// [`CallNote::extra`] under its own name. A field named as one of
    /// the core's ([`CALL_KEYS`]) is left out, and so is one that is
    /// `null`.
    pub fn add_to(&self, call: &mut Value) {
        let Some(call) = call.as_object_mut() else {
            return;
        };
        if let Some(cost) = self.cost_milli {
            call.insert("cost_milli".into(), json!(cost));
        }
        for (name, value) in &self.extra {
            if !value.is_null() && !CALL_KEYS.contains(&name.as_str()) {
                call.insert(name.clone(), value.clone());
            }
        }
    }
}

/// What a session made of a call: what the script is answered, who
/// journals it, and what the run shows of it.
///
/// An answer is built through a constructor, one for each way a call is
/// journaled, and read through its methods. Its fields are not public,
/// so an answer of another kind (a call not answered yet, the design's
/// "Calls that wait") is one more constructor.
///
/// | Constructor | The call | Journaled |
/// |---|---|---|
/// | [`Answer::of`] | A read; an operation whose value a replay needs (`draft.rebase`) | By the core, with the value or the failure |
/// | [`Answer::outcome`] | An operation performed again on replay (`files.*`, `draft.check`) | By the core, without the value |
/// | [`Answer::refused`] | A refusal or a failure nothing was written for: a rule of the run's, a write its plan refused | By the core, the failure |
/// | [`Answer::written`] | A write journaled in its own transaction; a call journaled with its charge | By the provider, before it answers |
/// | [`Answer::unwritten`] | A failure of the provider's own transaction, which left no row and no effect | By no one: the call is made again when its step is |
/// | [`Answer::waiting`] | A call of an operation tagged `@waits` that the provider holds and has no answer for yet | By no one now ([`StepSession::call`]) |
///
/// A call answered from its entry ([`Call::replay`]) is journaled by
/// no one, so its constructor is not read for that. It matters for the
/// charge alone: an answer that says what the call charged when it was
/// first made is built with `written`, the one constructor that may be
/// [`Answer::charged`]; any other replayed answer may be built with
/// `of`.
///
/// Given a failure, `of`, `outcome` and `refused` write the same row:
/// `refused` is the spelling for a failure, and the other two differ
/// only in what they keep of a value.
///
/// What the constructors do not let a provider say:
///
/// - **A charge the core would journal.** A call that charged more than
///   nothing is journaled by its provider, in the transaction that
///   charged it: the core's row keeps no cost, so a replay would show
///   none. [`Answer::charged`] is for an answer made with
///   [`Answer::written`]; any answer may be [`Answer::uncharged`]. A
///   charge above 0 on an answer the core journals fails an assertion
///   in a debug build, and otherwise the core drops it and logs the
///   provider's mistake ([`perform`]).
/// - **An unjournaled refusal of an operation the binding lacks.**
///   Whatever [`SurfaceProvider::undeclared`] answers is journaled by
///   the core as a refusal ([`perform`]).
/// - **That a call waits, of an operation that is not tagged `@waits`,
///   or of a call that came with its entry.** The core refuses the
///   call `provider_error` in the answer's place ([`perform`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    result: Result<Value, HostError>,
    journal: Journal,
    note: CallNote,
    /// For a waiting answer: when the call's wait ends if nothing has
    /// answered it.
    deadline: Option<chrono::DateTime<chrono::Utc>>,
}

impl Answer {
    fn new(result: Result<Value, HostError>, journal: Journal) -> Answer {
        Answer {
            result,
            journal,
            note: CallNote::default(),
            deadline: None,
        }
    }

    /// `result`, for the core to journal with its value.
    #[must_use]
    pub fn of(result: Result<Value, HostError>) -> Answer {
        Answer::new(result, Journal::Answer)
    }

    /// `result`, for the core to journal without its value: of an
    /// operation the provider performs again when its step is run again.
    #[must_use]
    pub fn outcome(result: Result<Value, HostError>) -> Answer {
        Answer::new(result, Journal::Outcome)
    }

    /// A refusal, or a failure that left nothing written, for the core
    /// to journal.
    #[must_use]
    pub fn refused(e: HostError) -> Answer {
        Answer::new(Err(e), Journal::Outcome)
    }

    /// `result`, which the provider has journaled itself: in the
    /// transaction of the write the call made, or with what it charged.
    #[must_use]
    pub fn written(result: Result<Value, HostError>) -> Answer {
        Answer::new(result, Journal::Written)
    }

    /// A failure that left nothing written, for neither side to
    /// journal: the provider's own transaction, which would have held
    /// the call's row with its effect, could not be made. The script is
    /// answered `e`, the place keeps no row, and the step executed
    /// again makes the call again (the module documentation, "Who
    /// journals a call").
    #[must_use]
    pub fn unwritten(e: HostError) -> Answer {
        Answer::new(Err(e), Journal::Unwritten)
    }

    /// No answer yet: the provider holds the call under its key
    /// ([`CallPlace::key`]), and its answer comes later. Only for a call
    /// of an operation whose definition says `@waits` that came with no
    /// entry; what the provider promises by it is said on
    /// [`StepSession::call`]. No one journals it, and the script is told
    /// nothing: [`Answer::result`] of a waiting answer is a failure no
    /// script is thrown, and whoever is handed the answer asks
    /// [`Answer::is_waiting`] first.
    #[must_use]
    pub fn waiting() -> Answer {
        Answer::new(
            Err(HostError::new(CODE_WAITING, "the call has no answer yet")),
            Journal::Waiting,
        )
    }

    /// [`Answer::waiting`], for a call whose wait ends at `deadline`
    /// if nothing has answered it by then (`None`: it has no deadline).
    /// The run loop parks the run until then: at the deadline it has
    /// the call's failure journaled, as whoever keeps the call says it
    /// (`Store::expire_calls`), and the run goes on.
    #[must_use]
    pub fn waiting_until(deadline: Option<chrono::DateTime<chrono::Utc>>) -> Answer {
        Answer {
            deadline,
            ..Answer::waiting()
        }
    }

    /// When a waiting call's wait ends ([`Answer::waiting_until`]);
    /// `None` for one with no deadline, and for any other answer.
    #[must_use]
    pub fn deadline(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.deadline
    }

    /// Whether this is [`Answer::waiting`]: the call has no answer yet.
    #[must_use]
    pub fn is_waiting(&self) -> bool {
        self.journal == Journal::Waiting
    }

    /// This answer, naming `target`.
    #[must_use]
    pub fn naming(mut self, target: Option<String>) -> Answer {
        self.note.target = target;
        self
    }

    /// This answer, of a call that charged `cost_milli` millicredits,
    /// or, answered from its entry, that charged them when it was first
    /// made. Only for an answer the provider journals
    /// ([`Answer::written`]): the charge and the row are written
    /// together, so the row says what the call cost. A charge above 0
    /// on any other answer is the provider's mistake; the core's row
    /// would not keep it.
    ///
    /// # Panics
    /// In a build with debug assertions, for a charge above 0 on an
    /// answer the core journals.
    #[must_use]
    pub fn charged(mut self, cost_milli: i64) -> Answer {
        debug_assert!(
            cost_milli <= 0 || self.journal == Journal::Written,
            "a call that charged is journaled by its provider, with its charge"
        );
        self.note.cost_milli = Some(cost_milli);
        self
    }

    /// This answer, of a call of a charged operation that charged
    /// nothing: the run shows a cost of 0. For any answer.
    #[must_use]
    pub fn uncharged(mut self) -> Answer {
        self.note.cost_milli = Some(0);
        self
    }

    /// This answer as the refusal of an operation a binding does not
    /// have: journaled by the core as an outcome whatever its
    /// constructor said, and `refusal` when it is no failure: a value,
    /// or that the call waits.
    fn into_refusal(mut self, refusal: impl FnOnce() -> HostError) -> Answer {
        if self.result.is_ok() || self.is_waiting() {
            self.result = Err(refusal());
        }
        self.journal = Journal::Outcome;
        self
    }

    /// This answer, with `extra` as the provider's own fields of the
    /// call ([`CallNote::extra`]).
    #[must_use]
    pub fn showing(mut self, extra: Fields) -> Answer {
        self.note.extra = extra;
        self
    }

    /// What the script is answered.
    pub fn result(&self) -> &Result<Value, HostError> {
        &self.result
    }

    /// What the script is answered, owned.
    ///
    /// # Errors
    /// The call's failure.
    pub fn into_result(self) -> Result<Value, HostError> {
        self.result
    }

    /// Who journals the call. Not read for a replayed call.
    #[must_use]
    pub fn journal(&self) -> Journal {
        self.journal
    }

    /// What the run shows of the call.
    #[must_use]
    pub fn note(&self) -> &CallNote {
        &self.note
    }
}

/// What the core answers a call to an operation the run's binding does
/// not have, when the provider has no words of its own for it
/// ([`SurfaceProvider::undeclared`]).
#[must_use]
pub fn not_declared(op: &str) -> HostError {
    HostError::new(
        CODE_UNKNOWN_OP,
        format!("clax.{op} is not an operation this run has"),
    )
}

/// What the core answers a call to `op` when no binding of the run
/// takes it ([`route`]), or the surface it is routed to has no such
/// operation: `unknown_op`, in the words `api::check_args` has for a
/// name that is no operation.
#[must_use]
pub fn no_such_operation(op: &str) -> HostError {
    HostError::new(CODE_UNKNOWN_OP, format!("clax.{op} is not an operation"))
}

/// What the core answers a script's call at place `seq` when the step
/// has no place for it ([`StepCx::place`]): the places of a script's
/// calls end where those of the bindings' `ending`s begin. The call is
/// neither journaled nor performed, on any attempt; it is told as any
/// refusal of the core's is (the step's `calls`, the events, a
/// `bot.call` record) and counted on the requests line.
///
/// This is a backstop, and a model should never meet it. The limit a
/// script is meant to meet is the sandbox's, which ends the script and
/// cannot be caught (`the script made more than its N host calls`,
/// `RunSettings::script_max_calls`); this refusal has the same code,
/// is an error the script can catch, and is reached only when that
/// limit is set above [`NOTICE_SEQ`], which the setting behind it
/// allows today. Once `script.max_calls` is capped below
/// [`NOTICE_SEQ`] nothing reaches it.
#[must_use]
pub fn no_place(seq: i64) -> HostError {
    HostError::new(
        CODE_NO_PLACE,
        format!(
            "call {seq} is past the {NOTICE_SEQ} calls one script may make; do the rest in \
             another script"
        ),
    )
}

/// The host error code of a script's call a step has no place for
/// ([`no_place`]).
pub const CODE_NO_PLACE: &str = "call_limit";

/// Checks the arguments of a call to `op` (`raw`, the JSON array the
/// sandbox passes) against `definition`, the definition of the surface
/// the call was routed to, and returns them, one value for each
/// parameter, `null` for an optional one left out. For Clax's own
/// surface this is `api::check_args`.
///
/// # Errors
/// [`no_such_operation`] for a name the definition does not have;
/// `invalid_args` for a wrong count or a value of the wrong type,
/// naming the parameter.
pub fn check_args(definition: &Definition, op: &str, raw: &Value) -> Result<Vec<Value>, HostError> {
    let operation = definition
        .operation(op)
        .ok_or_else(|| no_such_operation(op))?;
    let Value::Array(given) = raw else {
        return Err(HostError::new(
            CODE_BAD_ARGS,
            format!("clax.{op}: arguments must be an array"),
        ));
    };
    operation
        .check_args(given)
        .map_err(|e| HostError::new(CODE_BAD_ARGS, e.message))
}

/// One binding of a run as a step that runs a script holds it: the
/// binding, its surface's provider, and the session the provider opened
/// for the step's attempt.
#[derive(Clone, Copy)]
pub struct Bound<'a> {
    pub binding: &'a Binding,
    pub provider: &'a dyn SurfaceProvider,
    pub session: &'a dyn StepSession,
}

impl std::fmt::Debug for Bound<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bound")
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}

/// The core's part of one call of a script (`op` on `raw`, the JSON
/// array the sandbox passes, at `place`), after it has found the call
/// to be the one its place recorded (`replay`, the place's entry when
/// the journal holds one: the core has checked that it is of the same
/// operation on the same arguments, and a build with debug assertions
/// checks it again here). In this order:
///
/// 1. the operation is routed to one of the step's bindings
///    ([`route`]); one no binding takes is refused
///    ([`no_such_operation`]), and so is one whose binding's provider
///    takes no call, with the provider's error
///    ([`SurfaceProvider::unavailable`]);
/// 2. the arguments are checked against the definition of that
///    binding's surface ([`SurfaceProvider::definition`],
///    [`check_args`]); arguments that do not hold are refused, whether
///    or not the binding has the operation;
/// 3. the call is performed ([`perform`]): refused when the binding
///    does not have the operation, and the session's otherwise.
///
/// A refusal of 1 or 2 is journaled by the core as an outcome, unless
/// the place is in the journal already; it names no target.
pub async fn dispatch(
    bound: &[Bound<'_>],
    claim: &Claim<'_>,
    place: &CallPlace,
    op: &str,
    raw: &Value,
    replay: Option<&CallEntry>,
) -> Answer {
    debug_assert!(
        replay.is_none_or(|e| e.op() == op && e.args_hash() == args_hash(raw)),
        "the entry of place {} is of another call than {op}: the core checks a replay before it \
         dispatches",
        place.seq
    );
    let checked = match route_by(bound, |b| b.binding, op) {
        None => Err(no_such_operation(op)),
        Some(to) => match to.provider.unavailable() {
            Some(away) => Err(away),
            None => check_args(to.provider.definition(), op, raw).map(|args| (to, args)),
        },
    };
    match checked {
        Ok((to, args)) => {
            let call = Call {
                place,
                op,
                args: &args,
                raw,
                replay,
            };
            perform(to.provider, to.session, to.binding, claim, call).await
        }
        Err(e) => {
            let answer = Answer::refused(e);
            if replay.is_none() {
                claim.record(place, op, raw, &answer);
            }
            answer
        }
    }
}

/// The last part of [`dispatch`], for a call that is routed to
/// `binding` and whose arguments hold:
///
/// - an operation the binding does not have ([`Binding::has`]) is
///   answered by [`SurfaceProvider::undeclared`], or [`not_declared`],
///   and `session` does not see it. The answer is journaled as a
///   refusal whatever constructor made it, and one that is no failure
///   is replaced by [`not_declared`]: the hook is given no place and no
///   claim, so it can have journaled nothing and performed nothing;
/// - any other is `session`'s to answer ([`StepSession::call`]);
/// - an answer that the call waits ([`Answer::waiting`]) is the
///   provider's mistake unless the definition of its surface tags the
///   operation `@waits` and the call came with no entry: the call is
///   refused `provider_error` ([`CODE_PROVIDER_ERROR`]) in its place,
///   journaled as a refusal. One that may wait is returned as it is,
///   journaled by no one: what becomes of the script is the caller's;
/// - an answer the core journals that says the call charged more than
///   nothing is the provider's mistake ([`Answer::charged`]): a debug
///   build fails an assertion, and otherwise the charge is dropped
///   from what the run shows of the call and the mistake is logged,
///   once for the process;
/// - the answer of a call that is not replayed is journaled as it says
///   ([`Claim::record`]).
pub async fn perform(
    provider: &dyn SurfaceProvider,
    session: &dyn StepSession,
    binding: &Binding,
    claim: &Claim<'_>,
    call: Call<'_>,
) -> Answer {
    let answer = if binding.has(call.op) {
        may_wait(session.call(call).await, provider, &call)
    } else {
        provider
            .undeclared(call.op, call.args)
            .unwrap_or_else(|| Answer::refused(not_declared(call.op)))
            .into_refusal(|| not_declared(call.op))
    };
    debug_assert!(
        answer.journal() == Journal::Written || answer.note().cost_milli().unwrap_or(0) <= 0,
        "clax.{}: a call that charged is journaled by its provider, with its charge",
        call.op
    );
    let answer = without_a_charge_the_core_would_journal(answer, call.op);
    if call.replay.is_none() {
        claim.record(call.place, call.op, call.raw, &answer);
    }
    answer
}

/// `answer`, a session's to `call`, unless it says the call waits and
/// may not: only an operation its surface's definition tags `@waits`
/// waits, and only a call that came with no entry. Any other is refused
/// in the answer's place, for the core to journal.
fn may_wait(answer: Answer, provider: &dyn SurfaceProvider, call: &Call<'_>) -> Answer {
    if !answer.is_waiting() {
        return answer;
    }
    let tagged = provider
        .definition()
        .operation(call.op)
        .is_some_and(|o| o.op.waits);
    let why = match (tagged, call.replay.is_some()) {
        (true, false) => return answer,
        (false, _) => "is not tagged @waits",
        (true, true) => "was answered already",
    };
    Answer::refused(HostError::new(
        CODE_PROVIDER_ERROR,
        format!(
            "clax.{}: its surface answered that the call waits, and it {why}",
            call.op
        ),
    ))
    .naming(answer.note.target)
}

/// `answer` to a call to `op`, without a charge above 0 when the core
/// journals it: the core's row keeps no cost, so the run would show a
/// charge that a replay of the call does not. The charge is dropped
/// from the answer's note, and the provider's mistake is logged the
/// first time the process meets one.
fn without_a_charge_the_core_would_journal(mut answer: Answer, op: &str) -> Answer {
    static LOGGED: std::sync::Once = std::sync::Once::new();
    if answer.journal != Journal::Written && answer.note.cost_milli.unwrap_or(0) > 0 {
        LOGGED.call_once(|| {
            tracing::warn!(
                "clax-bot: clax.{op} answered a charge of {:?} millicredits on an answer the \
                 run loop journals; the charge is not shown (a call that charges is journaled \
                 by its provider)",
                answer.note.cost_milli
            );
        });
        answer.note.cost_milli = None;
    }
    answer
}

// ---------------------------------------------------------------------------
// What several bindings add to one step
// ---------------------------------------------------------------------------

/// The names the core gives fields of a step's payload where a
/// provider's fields are merged into it. A script step: `tool_use_id`,
/// `title`, `code`, `outcome`, `result`, `calls` (`script.rs`,
/// `Tick::script`). A terminal step (`run.rs`): `reason` and `error` of
/// a stop, with `title`, `code`, `outcome` and `calls` when a cancelled
/// script stopped the run; `summary` of a finish of a kind of `clax`,
/// `result` of a finish of any other kind (`crate::finish`); `code`
/// and `reason` of a give-up; `reason` and `error` of a failure.
pub const PAYLOAD_KEYS: [&str; 9] = [
    "calls",
    "code",
    "error",
    "outcome",
    "reason",
    "result",
    "summary",
    "title",
    "tool_use_id",
];

/// The names the core gives fields of what a model is told of a script
/// (`tool_result_content`, `script.rs`): `value`, `logs`,
/// `calls`, `error`, `replay`.
pub const TOOL_RESULT_KEYS: [&str; 5] = ["calls", "error", "logs", "replay", "value"];

/// The names `clax`'s provider puts at the top of a step's payload
/// ([`merge_fields`]): a script step's session's (`changed`, `replied`,
/// `published`, `created`, `base`) and its `ending`'s (`notice`,
/// `posted`, `sources`).
pub const CLAX_FIELDS: [&str; 8] = [
    "base",
    "changed",
    "created",
    "notice",
    "posted",
    "published",
    "replied",
    "sources",
];

/// A provider answered a field under a name it may not use
/// ([`merge_fields`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldClash {
    /// The surface whose provider answered it.
    pub surface: String,
    /// The name, at the top of what the fields were merged into.
    pub field: String,
    /// Whether the name is one of the core's own; else it was taken
    /// already, by another binding's fields.
    pub core: bool,
}

impl std::fmt::Display for FieldClash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let FieldClash {
            surface,
            field,
            core,
        } = self;
        if *core {
            write!(
                f,
                "the provider of {surface} answered a field named {field}, which is the run loop's own"
            )
        } else {
            write!(
                f,
                "the provider of {surface} answered a field named {field}, which the step already has"
            )
        }
    }
}

impl std::error::Error for FieldClash {}

/// Which surface made each name at the top of what [`merge_fields`]
/// merges into: one for a step's payload, and one for what its model
/// is told, kept by the core for as long as it merges into them.
///
/// It is what tells a surface's own object, which its later fields
/// join, from a field of the same name that another binding put there
/// (`clax`'s `published` is an object too): the JSON alone cannot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    /// The surface that made each top-level name.
    made: BTreeMap<String, String>,
}

impl Merged {
    /// Nothing merged yet.
    #[must_use]
    pub fn new() -> Merged {
        Merged::default()
    }

    /// The surface whose merge made the top-level name `name`, when a
    /// merge made it.
    #[must_use]
    pub fn by(&self, name: &str) -> Option<&str> {
        self.made.get(name).map(String::as_str)
    }
}

/// Merges `fields`, which the provider of `surface` answered (a
/// session's [`StepParts::payload`] or [`StepParts::tool_result`], an
/// [`SurfaceProvider::ending`]'s fields), into `into`, where the core
/// keeps its own fields under the names `core` ([`PAYLOAD_KEYS`],
/// [`TOOL_RESULT_KEYS`]). `merged` is what was merged into `into`
/// before ([`Merged`]), and is told what this merge makes:
///
/// - the fields of `clax` go in under their own names, each at the top
///   of `into`, as today's payloads have them;
/// - the fields of any other surface go into one object, under the
///   surface's name at the top of `into`: made by the surface's first
///   merge that has fields, and added to by its later ones (in the
///   `stopped` step of a cancelled script, a session's fields and then
///   its `ending`'s). No fields, nothing.
///
/// The verdict does not depend on the order the bindings are merged
/// in: a name two of them want at the top clashes for whichever comes
/// second. So a surface whose name is one of `core`, or one of the
/// names `clax` puts at the top, clashes on every step both add fields
/// to. A host's surface is refused those names where it is defined
/// (`clax_surface::STEP_FIELDS`).
///
/// # Errors
/// [`FieldClash`], and `into` and `merged` are left as they were:
///
/// - a name to be added at the top of `into` that is one of `core`;
/// - a name to be added at the top of `into` that is there already,
///   unless it is the surface's own object, made by an earlier merge of
///   the same surface (`merged` says so, not the shape of what is
///   there);
/// - a field of another surface that its own object already has: a
///   provider that answers one of its fields twice in a step.
///
/// The provider is at fault. The core never drops the field and never
/// lets it overwrite: it abandons the attempt and counts it against
/// the step as a commit that failed (`Store::note_commit_failure`), so
/// a provider that clashes on every attempt ends the run `failed` at
/// `RunSettings::max_commit_failures` and cannot hold it open. In the
/// `failed` step itself the clash is logged, none of that binding's
/// fields are merged, the other bindings' fields are kept, and the
/// step commits.
pub fn merge_fields(
    into: &mut Fields,
    merged: &mut Merged,
    core: &[&str],
    surface: &str,
    fields: Fields,
) -> Result<(), FieldClash> {
    if fields.is_empty() {
        return Ok(());
    }
    let clash = |field: &str, core: bool| FieldClash {
        surface: surface.to_string(),
        field: field.to_string(),
        core,
    };
    if surface == CLAX_SURFACE {
        for name in fields.keys() {
            let is_core = core.contains(&name.as_str());
            if is_core || into.contains_key(name) {
                return Err(clash(name, is_core));
            }
        }
        for name in fields.keys() {
            merged.made.insert(name.clone(), surface.to_string());
        }
        into.extend(fields);
        return Ok(());
    }
    if core.contains(&surface) {
        return Err(clash(surface, true));
    }
    let own = merged.by(surface) == Some(surface);
    match into.get_mut(surface) {
        None => {
            into.insert(surface.to_string(), Value::Object(fields));
            merged.made.insert(surface.to_string(), surface.to_string());
        }
        Some(Value::Object(object)) if own => {
            if let Some(taken) = fields.keys().find(|name| object.contains_key(*name)) {
                return Err(clash(&format!("{surface}.{taken}"), false));
            }
            object.extend(fields);
        }
        Some(_) => return Err(clash(surface, false)),
    }
    Ok(())
}

/// What the sessions of one step add to its commit, joined: the fields
/// of the step's payload, the fields of what its model is told, and the
/// rows its commit writes. One is made for each attempt of a step,
/// before the first session's parts are joined.
///
/// It holds the two [`Merged`] of the step, one for the payload and one
/// for what the model is told, each beside the fields it is of, so that
/// neither can be handed to the other's merge nor kept from one step to
/// the next.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepJoin {
    payload: Fields,
    payload_made: Merged,
    told: Fields,
    told_made: Merged,
    rows: Vec<DraftChange>,
}

impl StepJoin {
    /// Nothing joined yet.
    #[must_use]
    pub fn new() -> StepJoin {
        StepJoin::default()
    }

    /// Joins `parts`, which the session of the binding of `surface`
    /// answered ([`StepSession::finish`]): its payload fields over
    /// [`PAYLOAD_KEYS`], its `tool_result` fields over
    /// [`TOOL_RESULT_KEYS`] ([`merge_fields`]), and its rows after those
    /// joined before.
    ///
    /// # Errors
    /// [`FieldClash`], the provider's fault: what was joined before is
    /// as it was, and nothing of `parts` is joined. The run loop
    /// abandons the attempt and counts it against the step as a commit
    /// that failed.
    pub fn add(&mut self, surface: &str, parts: StepParts) -> Result<(), FieldClash> {
        let StepParts {
            payload,
            tool_result,
            draft,
        } = parts;
        let (mut fields, mut made) = (self.payload.clone(), self.payload_made.clone());
        merge_fields(&mut fields, &mut made, &PAYLOAD_KEYS, surface, payload)?;
        merge_fields(
            &mut self.told,
            &mut self.told_made,
            &TOOL_RESULT_KEYS,
            surface,
            tool_result,
        )?;
        (self.payload, self.payload_made) = (fields, made);
        self.rows.extend(draft);
        Ok(())
    }

    /// The fields the sessions add to what the model is told of the
    /// script. The core joins them to its own before it trims the
    /// result.
    #[must_use]
    pub fn told(&self) -> &Fields {
        &self.told
    }

    /// The step's payload: `core`, the run loop's own fields (each named
    /// in [`PAYLOAD_KEYS`]), with the sessions' beside them.
    #[must_use]
    pub fn payload(&self, core: Fields) -> Value {
        debug_assert!(
            core.keys().all(|k| PAYLOAD_KEYS.contains(&k.as_str())),
            "a field of the run loop's own that `PAYLOAD_KEYS` does not name"
        );
        let mut payload = self.payload.clone();
        payload.extend(core);
        Value::Object(payload)
    }

    /// The step's commit: `payload` ([`StepJoin::payload`]), what the
    /// step charges and its audit records, with the rows the sessions
    /// answered.
    #[must_use]
    pub fn commit(
        self,
        payload: Value,
        usage: Vec<Metered>,
        events: Vec<AuditRecord>,
    ) -> StepCommit {
        StepCommit {
            payload,
            usage,
            events,
            draft: self.rows,
            reason: None,
        }
    }
}

// ---------------------------------------------------------------------------
// A session
// ---------------------------------------------------------------------------

/// How a step that ran a script ends, for [`StepSession::finish`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepEnd {
    /// The step commits with what the script did.
    Commit,
    /// The script was cancelled: the step becomes the run's `stopped`
    /// step. What the session's calls wrote outside the step stands;
    /// what waited for the step's commit is not kept.
    Cancelled,
}

/// What one binding's session adds to its step's commit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepParts {
    /// Fields of the step's payload. For `clax`: `changed`, `replied`,
    /// `published`, `created`, `base`; after [`StepEnd::Cancelled`],
    /// `published`, `replied` and `base` alone. Later steps' sessions
    /// and the ending hooks read them in [`StepCx::history`]: `clax`'s
    /// at the top of the payload, another surface's under the surface's
    /// name ([`merge_fields`]). No field is named as one of the core's
    /// ([`PAYLOAD_KEYS`]).
    pub payload: Fields,
    /// Fields of what the model is told of the script, beside its value,
    /// logs and calls. For `clax`: `published`, when the script
    /// published. Joined as the payload's are, with no field named as
    /// one of the core's ([`TOOL_RESULT_KEYS`]); the core trims what it
    /// tells the model after it has joined them.
    pub tool_result: Fields,
    /// Rows the step's commit writes in its transaction
    /// (`StepCommit::draft`): today the changes to the run's draft, the
    /// one kind of row a step commits for a provider. Empty after
    /// [`StepEnd::Cancelled`].
    pub draft: Vec<DraftChange>,
}

/// One attempt of one step, for one binding. See the module
/// documentation for what each of today's operations does in one.
///
/// A session keeps the provider's state of the step in memory; nothing
/// it holds is stored until the step commits, so dropping it leaves
/// nothing behind but what its calls wrote under their keys, which the
/// next attempt's calls are answered from. Called again with the calls
/// of an earlier attempt, each with its journal entry, a session ends
/// in the state that attempt's session ended in, and `finish` answers
/// the same parts.
///
/// A session checks for itself what it must not do whoever calls it: an
/// operation its binding does not have is refused by the core before
/// the session sees it ([`dispatch`]), and refused again by a session
/// that is handed one all the same.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait StepSession: MaybeSendSync {
    /// Performs `call`, or answers it from its entry
    /// ([`Call::replay`]), and says who journals it (the [`Answer`]'s
    /// constructor).
    ///
    /// A write is made under [`CallPlace::key`]: made again with the
    /// same key it takes effect once and answers the same. A read
    /// answers what the provider holds now; it is the journal that makes
    /// a replayed read answer what it answered before. A refusal is an
    /// answer like any other.
    ///
    /// **Calls never overlap.** The calls of a step are made one at a
    /// time, in the order of their places, across all of the step's
    /// sessions: the sandbox sends a script's calls so, and the core
    /// awaits each answer before it makes the next call. A session may
    /// rely on it (`clax`'s `draft.rebase` takes a copy of the draft,
    /// awaits the store, and puts the copy back, which would lose a
    /// write made in between), and need not guard its state against a
    /// second call running beside the first.
    ///
    /// **The call's future may be dropped at any await**, and then
    /// never polled again: when the script is cancelled, and when the
    /// driver loses its claim on the run. After a cancel
    /// [`StepSession::finish`] is called with [`StepEnd::Cancelled`] on
    /// this session; after a lost claim the session is dropped. So at
    /// every await a session's state is either as it was before the
    /// call or as it is after it, never between: nothing half-applied
    /// that `finish` would then answer as the step's parts. What the
    /// call had already written under its key stands, and is the next
    /// attempt's to be answered from.
    ///
    /// **A call that waits.** For a call of an operation its
    /// definition tags `@waits` that came with no entry, a session may
    /// answer [`Answer::waiting`] in place of a value or a failure. By
    /// it the provider promises:
    ///
    /// - it holds the call under [`CallPlace::key`]: whatever the call
    ///   does (a question asked, an effect begun) was begun once, and a
    ///   call made again with the key is that call, not another, also
    ///   for an operation tagged `@once`;
    /// - it has written no row at the call's place and will write none
    ///   under this attempt's claim, which the run loop gives up;
    /// - the call's answer comes later, a value or a failure (its
    ///   deadline's among them), by one of two ways. Whoever brings it
    ///   journals it at the call's place, as the row the core writes
    ///   for [`Answer::of`], and the step run again makes the call with
    ///   that entry ([`Call::replay`]), which the session answers from
    ///   it as it answers any replayed call. Or the step is run again
    ///   before any row is there: the call is made again under its key,
    ///   and the session answers it then, with the answer when it has
    ///   one, and that it waits when it has not;
    /// - how long it waits inside this method before it answers so is
    ///   its own to decide (the environment's policy says:
    ///   [`crate::policy::Delivery::park_after`]). The core never drops
    ///   the future of a call to make it wait.
    ///
    /// The session's state is as it was before the call: a waiting
    /// call adds nothing to what [`StepSession::finish`] answers, and
    /// the attempt it was made in is not committed.
    async fn call(&self, call: Call<'_>) -> Answer;

    /// Ends the session: what the step's commit must include of it.
    /// Called once, after the last of the session's calls has returned
    /// or been dropped.
    ///
    /// It writes nothing and has no effect outside the value it
    /// answers. It may be called by a driver that no longer holds the
    /// run: the run loop drops a session without `finish` when it knows
    /// its claim is lost, but a claim can lapse unnoticed, and then the
    /// only guard is the store, which refuses the step's commit
    /// (`run_not_claimed`). So nothing a session does in `finish` may
    /// rest on the claim.
    fn finish(self: Box<Self>, end: StepEnd) -> StepParts;
}

// ---------------------------------------------------------------------------
// A run's end
// ---------------------------------------------------------------------------

/// How a run ends.
#[derive(Clone, Debug, PartialEq)]
pub enum End {
    /// The model called `finish`. The kind's result is given beside it,
    /// to the surface that declared the kind.
    Finished,
    /// The model gave up (`model`), or the run loop did for it
    /// (`refusal`, `context_window`, `no_tool_call`).
    GaveUp { code: String, reason: String },
    /// The run was stopped: `cancelled`, `request_gone`, `disabled`,
    /// `no_flag`, `credits_exhausted`, `not_configured`,
    /// `model_not_priced`, `run_cap`, `key_rejected`, `model_error`.
    Stopped {
        reason: String,
        /// For `run_cap`: the limit the run reached, with its value, as
        /// the environment's policy states it (`40 steps`,
        /// `200 credits`, `30 minutes`).
        limit: Option<String>,
        /// For `credits_exhausted`: when the person's credits reset.
        credits_reset: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// The run could not go on: `internal_error`, a step that could not
    /// be committed.
    Failed { code: String },
}

impl End {
    /// A stop for `reason` with no limit and no reset to tell.
    #[must_use]
    pub fn stopped(reason: impl Into<String>) -> End {
        End::Stopped {
            reason: reason.into(),
            limit: None,
            credits_reset: None,
        }
    }

    /// The status the run ends in.
    #[must_use]
    pub fn status(&self) -> RunStatus {
        match self {
            End::Finished => RunStatus::Finished,
            End::GaveUp { .. } => RunStatus::GaveUp,
            End::Stopped { .. } => RunStatus::Stopped,
            End::Failed { .. } => RunStatus::Failed,
        }
    }

    /// The kind of the terminal step.
    #[must_use]
    pub fn step_kind(&self) -> StepKind {
        match self {
            End::Finished => StepKind::Finish,
            End::GaveUp { .. } => StepKind::GiveUp,
            End::Stopped { .. } => StepKind::Stopped,
            End::Failed { .. } => StepKind::Failed,
        }
    }

    /// Why the run ended, as the run keeps it (`Run::reason`): `None`
    /// for a run that finished.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            End::Finished => None,
            End::GaveUp { code, .. } | End::Failed { code } => Some(code),
            End::Stopped { reason, .. } => Some(reason),
        }
    }
}

// ---------------------------------------------------------------------------
// A provider
// ---------------------------------------------------------------------------

/// What provides one surface to the runs bound to it. The run loop
/// holds one for each binding of a run. Async and, on wasm32, not
/// `Send`, as [`crate::host::BotHost`] is.
///
/// Every method may be called again for the same run and step: a step
/// begun and not committed is executed again.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait SurfaceProvider: MaybeSendSync {
    /// The definition of the surface this provides: what a binding's
    /// operations are taken from ([`Binding::from_stored`]) and what
    /// the arguments of a call routed to the surface are checked
    /// against ([`check_args`]). The same for as long as the provider
    /// lives.
    fn definition(&self) -> &Definition;

    /// Whether every run bound to the surface has operation `op` of its
    /// file, whatever the run was granted: the surface's own rule, which
    /// a stored binding does not hold ([`Binding::from_stored`]). Every
    /// operation, unless the provider says otherwise; for `clax`, every
    /// operation but the web's, which a run has by its grant.
    fn standing(&self, _op: &str) -> bool {
        true
    }

    /// The prompt file the run was started with, as text, when its
    /// surface has one of its own: what a run of one of the surface's
    /// kinds is sent in place of the standard prompt
    /// (`prompt::system_of`, `prompt::request_of`, `own`). `Ok(None)`,
    /// unless the provider says otherwise. Asked of the provider of the
    /// surface that declared the run's kind, by the run's first turn.
    ///
    /// # Errors
    /// Why the run's prompt file cannot be given: the run is never sent
    /// another in its place. A failure that is not retryable ends the
    /// run `failed` under its code
    /// ([`crate::registry::CODE_PROMPT_GONE`]: the text is no longer
    /// kept); a retryable one (it could not be read now) fails the tick,
    /// which is made again.
    fn prompt(&self) -> Result<Option<&str>, HostError> {
        Ok(None)
    }

    /// What a script is answered when it calls `op` (on checked `args`),
    /// an operation of the surface's file that the run's binding does
    /// not have. This is not a call: it has no place and no key, and
    /// nothing may take effect. `None` for the core's own refusal
    /// ([`not_declared`]). The core journals the answer as a refusal,
    /// so it is one made with [`Answer::refused`]: an answer made
    /// otherwise is journaled as one all the same, and one that is no
    /// failure is replaced by the core's own refusal ([`perform`]).
    fn undeclared(&self, _op: &str, _args: &[Value]) -> Option<Answer> {
        None
    }

    /// Why no call reaches this provider, when none can: its surface is
    /// bound and nothing provides it here (a host that is away). `None`
    /// for a provider that takes calls, which every provider is unless
    /// it says otherwise. A script's call routed to a binding whose
    /// provider answers an error is refused with it before its
    /// arguments are checked, journaled by the core as an outcome, and
    /// no session sees it ([`dispatch`]).
    fn unavailable(&self) -> Option<HostError> {
        None
    }

    /// The provider a tick of the run whose row is `row` holds in this
    /// one's place for the run's binding `binding`, as the row keeps
    /// it: for a provider that wants the row as the tick read it (for
    /// `clax`, whose own columns are in it), so that it need not read
    /// the row again, and for one that serves several registrations of
    /// one surface's name, which chooses by the binding's registration
    /// and definition hash. `None`, unless the provider says otherwise:
    /// the tick holds this provider itself. Asked once a tick for each
    /// binding of the surface, before any other method; the provider
    /// answered is asked nothing after the tick. The binding's
    /// operations are then taken from the provider answered
    /// ([`Binding::as_stored`]). It is not asked for a run that has no
    /// binding of the surface that declared its kind: that run's kind
    /// is served by the provider registered under the name.
    fn for_run(&self, _row: &Run, _binding: &BindingAsStored) -> Option<Arc<dyn SurfaceProvider>> {
        None
    }

    /// What the audit records of a run bound to the surface by
    /// `binding` are filed on: the IDs of the objects the run concerns
    /// (for `clax`, the artifact of the binding's scope). `None`,
    /// unless the provider says otherwise: the surface names no object.
    /// Each binding of a run is asked once a tick, the surface that
    /// declared the run's kind first, and the first answer is the
    /// run's subject; a run none of whose bindings names one is filed
    /// on its person alone.
    fn audit_subject(&self, _binding: &Binding) -> Option<AuditIds> {
        None
    }

    /// The request of `run`, a run of one of this surface's kinds,
    /// resolved: the value the run's first turn is sent as its request,
    /// made from the request as stored ([`RunInfo::request`]) and what
    /// it names as the surface holds it now (for `clax`, the artifact
    /// and the threads' comments, or the brief). The request as stored,
    /// unless the provider says otherwise. Asked of the provider of the
    /// surface that declared the run's kind, by each attempt of the
    /// run's first turn; the run loop renders what is answered.
    ///
    /// # Errors
    /// What the request names is gone, or cannot be read: the run stops
    /// (`request_gone`).
    async fn request(&self, run: &RunInfo) -> Result<Value, HostError> {
        Ok(run.request.clone())
    }

    /// A driver has started: the provider does what it owes the runs
    /// no tick will come for (for `clax`, it starts the follow-ups
    /// queued on threads no run works on). Called once for each
    /// provider registered, after the repeats of `ended`
    /// ([`SurfaceProvider::awaiting_ended`]) and before the driver
    /// queues the runs that have not ended, so a run started here is
    /// resumed with them. Answers how many runs it started; none,
    /// unless the provider says otherwise.
    ///
    /// # Errors
    /// The provider's: logged, and the driver starts all the same.
    async fn driver_started(&self) -> clax_core::Result<usize> {
        Ok(0)
    }

    /// `run` is being worked on: called by each tick that is about to
    /// take a turn or run a script, before that step is begun, on every
    /// attempt. The tick may still end the run instead (it finds no
    /// key, the turn has no time or credits left, the request is gone,
    /// the model answers an error, the run is cancelled), so the call
    /// does not say that a step which keeps the run going follows. The
    /// provider renews whatever it shows of a run at work (for `clax`,
    /// the roster record on the run's threads); a run whose ticks stop
    /// coming is no longer renewed. Nothing is answered.
    async fn working(&self, _run: &RunInfo) {}

    /// Opens the session of the attempt `cx` describes, for a step that
    /// runs a script. A provider loads here what the step works on, as
    /// the committed steps left it ([`StepCx::history`]).
    ///
    /// # Errors
    /// What keeps the step from starting: the tick fails as for a
    /// store error, the sessions the step's other bindings had opened
    /// are dropped, and the next tick begins the step again, with an
    /// attempt one higher, and opens every session again. Nothing the
    /// failed call did may need undoing.
    ///
    /// The failure is not counted against the step: a `begin` that
    /// always fails keeps the step open for as long as the run's other
    /// limits allow, as a draft that cannot be loaded always did. (An
    /// `ending` that fails is counted; the two
    /// differ.)
    async fn begin<'s>(&'s self, cx: StepCx<'s>) -> clax_core::Result<Box<dyn StepSession + 's>>;

    /// The run is ending as `end` says: called inside the terminal step,
    /// begun and not committed, with the driver's claim. `result` is the
    /// kind's result, given to the surface that declared the kind when
    /// the run finished.
    ///
    /// It is called on each attempt of the terminal step, for every
    /// binding of the run, the kind's surface first; one binding's
    /// error does not keep another's from being called in that attempt.
    ///
    /// What the provider must do while the claim holds, it does here: a
    /// write is journaled at one of the binding's own places
    /// ([`StepCx::notice`], [`Claim::entry`]), so an attempt made again
    /// writes nothing more. It answers the fields the terminal step's
    /// payload keeps of it ([`merge_fields`]), the same on every
    /// attempt.
    ///
    /// # Errors
    /// What must keep the terminal step from committing. A write that
    /// could not be made is not one; the provider logs it and answers.
    ///
    /// An `Err` from any binding counts as a failed commit of the step
    /// does: the attempt is abandoned, the failure is counted against
    /// the step (`Store::note_commit_failure`), the tick fails, and a
    /// later tick begins the step again and calls every binding's
    /// `ending` again. A step whose failures reach
    /// `RunSettings::max_commit_failures` is given up, and the run ends
    /// `Failed` (`internal_error`), as it does today for a step that
    /// cannot be committed. So no provider keeps a run from ending.
    ///
    /// For [`End::Failed`] there is nothing further to fall back to: an
    /// `Err` is logged, the fields of that binding are left out, the
    /// other bindings' fields are kept, and the step commits. Fields
    /// that clash there ([`FieldClash`]) are treated the same. So is
    /// the stop of a cancelled script, which is taken in the tick that
    /// was cancelled, with what the script used charged: an `Err` or a
    /// clash there is counted against the step and that binding's
    /// fields are left out.
    async fn ending(
        &self,
        cx: StepCx<'_>,
        end: &End,
        result: Option<&Value>,
    ) -> clax_core::Result<Fields>;

    /// How the run ends for a registration that is to be told of it,
    /// for the terminal step's commit to keep with the run
    /// (`Store::commit_bot_step_on`, `ends`): the run ends and the end
    /// is kept in one transaction, so a tick that dies after its commit
    /// leaves no host untold. Asked on each attempt of the terminal
    /// step, after [`SurfaceProvider::ending`], with the same `end` and
    /// `result`. `None`, unless the provider says otherwise: nothing is
    /// kept for it, and what it does for the run's end it does in
    /// [`SurfaceProvider::ended`]. A provider that answers one names a
    /// registration the run's store holds: an end for a registration
    /// the store does not know refuses the commit.
    fn end_for(&self, _run: &RunInfo, _end: &End, _result: Option<&Value>) -> Option<EndFor> {
        None
    }

    /// The run ended as `end` says, and its terminal step is committed.
    /// The claim is gone, so nothing is journaled here. `result` is as
    /// for [`SurfaceProvider::ending`].
    ///
    /// It is called at least once, and never before the terminal
    /// commit: first by the tick that committed, and again, when a
    /// driver next starts, for a run the provider says it still awaits
    /// the end of ([`SurfaceProvider::awaiting_ended`]): after a crash
    /// between the commit and the call, or an `Err`.
    ///
    /// **What a repeat carries.** The same kind of end for the same
    /// run: the same variant of [`End`], and so the same status. Its
    /// details are best effort. The tick that committed has the whole
    /// value; a repeat made after a restart is rebuilt from the run's
    /// row and its terminal step, which keep the reason and the code
    /// but not [`End::Stopped`]'s `limit` (it is only inside the
    /// notice's text) nor its `credits_reset` (kept nowhere), and may
    /// lack `result`. A provider does for a repeat only what the kind
    /// and the run's ID need.
    ///
    /// **A repeat must be harmless.** What the provider did for the
    /// run's end the first time, it does not do twice. It need remember
    /// a run only until it has been told `ended` once (which is never
    /// before the run has ended); after that it may forget the run, and
    /// a repeat for a run it no longer knows is ignored, answering
    /// `Ok`.
    ///
    /// # Errors
    /// The provider's: the run has ended whatever it answers, and the
    /// run loop logs it.
    async fn ended(
        &self,
        run: &RunInfo,
        end: &End,
        result: Option<&Value>,
    ) -> Result<(), HostError>;

    /// The IDs of the runs whose end this provider still has something
    /// to do for, as far as it remembers: runs it was not yet told
    /// `ended` of, or was told and failed. Asked when a driver starts
    /// (`Driver::start`), which then calls [`SurfaceProvider::ended`]
    /// again for each of them that has ended, with the end rebuilt from
    /// what the store keeps of the run; a run named here that has not
    /// ended is left alone. So a tick that died between its terminal
    /// commit and its call of `ended` leaves nothing undone for longer
    /// than until the next driver starts.
    ///
    /// The provider is what remembers: the run loop keeps no record of
    /// which runs' providers were told. A provider with nothing left to
    /// do for a run need not name it, whether or not it was told (for
    /// `clax`, the runs named are the last run of each thread that has
    /// a follow-up queued and no run at work). None, unless the
    /// provider says otherwise: a provider that keeps nothing across a
    /// run's end is owed no repeat.
    ///
    /// # Errors
    /// The provider's: logged, and no run of its is told again by that
    /// driver's start.
    async fn awaiting_ended(&self) -> clax_core::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// FakeSurfaceProvider
// ---------------------------------------------------------------------------

/// The file of the surface [`FakeSurfaceProvider`] provides.
pub const FAKE_SURFACE: &str = include_str!("fake.d.ts");

static FAKE_DEFINITION: LazyLock<Definition> = LazyLock::new(|| {
    let reserved = Reserved::of(&[crate::api::definition()]);
    let host = Author::Host {
        reserved: &reserved,
    };
    clax_surface::parse(FAKE_SURFACE, host, &Limits::default())
        .unwrap_or_else(|e| panic!("crates/clax-bot/src/fake.d.ts: {e}"))
});

/// One call a [`FakeSurfaceProvider`]'s session was given.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeCall {
    pub key: String,
    pub op: String,
    pub args: Vec<Value>,
    /// Whether it came with its journal entry.
    pub replayed: bool,
}

/// One write a [`FakeSurfaceProvider`] applied.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeApplied {
    pub key: String,
    pub op: String,
    pub args: Vec<Value>,
    /// What it answered, and answers a repeat of the key.
    pub answer: Value,
}

/// Which hook a [`FakeHook`] records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook {
    Working,
    Begin,
    Finish,
    Ending,
    Ended,
}

/// One hook a [`FakeSurfaceProvider`] received, with what the store held
/// of the run when it was made.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeHook {
    pub hook: Hook,
    pub run: String,
    /// The step and attempt it was given; `None` for
    /// [`Hook::Working`] and [`Hook::Ended`], which are given neither.
    pub step: Option<(i64, i64)>,
    /// The run's status in the store (`None` for a run it does not
    /// hold).
    pub status: Option<RunStatus>,
    /// The end, for [`Hook::Ending`] and [`Hook::Ended`].
    pub end: Option<End>,
    /// The kind's result the hook was given, for [`Hook::Ending`] and
    /// [`Hook::Ended`].
    pub result: Option<Value>,
}

/// The state of a [`FakeSurfaceProvider`].
#[derive(Debug, Default)]
pub struct FakeSurfaceState {
    /// The values `fake.set` keeps, by name.
    pub values: BTreeMap<String, String>,
    /// How many times `fake.count` was performed.
    pub count: u64,
    /// Each write applied, in order: a key appears once.
    pub applied: Vec<FakeApplied>,
    /// Every call a session was given, repeats and replays included.
    pub calls: Vec<FakeCall>,
    /// Every hook received, in order.
    pub hooks: Vec<FakeHook>,
    /// An error to answer the next performed call of an operation with.
    pub fail_next: BTreeMap<String, HostError>,
    /// An error to answer the next performed call of an operation with
    /// as one that left nothing written ([`Answer::unwritten`]): the
    /// call does nothing, and no one journals it.
    pub unwritten_next: BTreeMap<String, HostError>,
    /// What `fake.ask` is answered, by its question. A question with
    /// no answer here is held, and its call waits
    /// ([`Answer::waiting`]).
    pub answers: BTreeMap<String, String>,
    /// The operations whose next performed call answers that it waits,
    /// once each, whatever the operation's tags say: the provider's
    /// mistake for one that is not tagged `@waits`.
    pub wait_next: Vec<String>,
    /// The hooks that fail the next time they are called, once each:
    /// [`Hook::Begin`] before it opens a session, [`Hook::Ending`]
    /// after it has written its notice.
    pub fail_hooks: Vec<Hook>,
    /// The runs whose end the fake acted on in `ended`: a run appears
    /// once, however many times it was told.
    pub followed: Vec<String>,
    /// What the fake answers a run's audit records are filed on
    /// ([`SurfaceProvider::audit_subject`]); `None`, on nothing.
    pub subject: Option<AuditIds>,
    /// How many times the fake was told that a driver started
    /// ([`SurfaceProvider::driver_started`]).
    pub drivers_started: usize,
    /// How many times the fake was asked for a run's request
    /// ([`SurfaceProvider::request`]).
    pub requests: usize,
}

/// A [`SurfaceProvider`] for tests, with state of its own and no store
/// write: the surface `fake` (`src/fake.d.ts`), a counter and a table
/// of values.
///
/// - `fake.count` reads a counter that moves each time it is performed,
///   and `fake.get` a value: reads, answered afresh each time.
/// - `fake.set` is a keyed write: it applies once for a key and answers a
///   repeat of the key what it answered first; a key used for another
///   call is `key_reused`.
/// - `fake.fail` fails (`broken`, retryable); `fake.refuse` refuses
///   (`forbidden`) and writes nothing.
/// - `fake.ask` is an effect that waits, tagged `@once`: it keeps the
///   question once for its key ([`FakeSurfaceState::applied`], with no
///   answer) and answers that the call waits
///   ([`Answer::waiting`]) until a test has supplied the question's
///   answer ([`FakeSurfaceState::answers`]), which a call made again
///   under the key is then answered.
/// - `fake.hidden` is an operation a test leaves out of its binding.
///
/// It journals nothing: every answer is the core's to journal
/// ([`Journal::Answer`]), as an outside provider's is, but one a test
/// asked to leave unwritten ([`FakeSurfaceState::unwritten_next`]). A call that
/// comes with its entry is answered from it and performs nothing. Its
/// session's part of a step is the names the step set
/// (`payload.fake_set`), from performed and replayed calls alike.
/// `ending` writes one notice under the key of the binding's first
/// notice place ([`StepCx::notice`]) and answers `fake_notice`. `ended`
/// acts on a run's end once ([`FakeSurfaceState::followed`]), however
/// many times it is told, and the fake awaits the end of each run it
/// was told is ending and not yet that it ended. A test makes `begin` or `ending` fail once
/// with [`FakeSurfaceState::fail_hooks`].
///
/// It records every call with its key and every hook with the run's
/// status in the store at that moment, and the end and the kind's
/// result `ending` and `ended` were given ([`FakeSurfaceState`]), and
/// [`FakeSurfaceProvider::lifecycle`] checks what the run loop owes a
/// provider.
pub struct FakeSurfaceProvider {
    pub store: Arc<Store>,
    pub state: Mutex<FakeSurfaceState>,
}

impl std::fmt::Debug for FakeSurfaceProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeSurfaceProvider")
            .finish_non_exhaustive()
    }
}

impl FakeSurfaceProvider {
    /// The name of the surface.
    pub const SURFACE: &'static str = "fake";
    /// The operation no binding [`FakeSurfaceProvider::binding`] makes
    /// has.
    pub const HIDDEN: &'static str = "fake.hidden";

    /// A provider that reads runs' statuses from `store`.
    #[must_use]
    pub fn new(store: Arc<Store>) -> FakeSurfaceProvider {
        FakeSurfaceProvider {
            store,
            state: Mutex::new(FakeSurfaceState::default()),
        }
    }

    /// The surface's definition, `src/fake.d.ts`, read as a host's: its
    /// name is none Clax's own surface reserves.
    ///
    /// # Panics
    /// When the file is not a definition; the crate's tests refuse that.
    #[must_use]
    pub fn definition() -> &'static Definition {
        &FAKE_DEFINITION
    }

    /// A binding of the surface with every operation but
    /// [`FakeSurfaceProvider::HIDDEN`]: what a stored binding of the
    /// surface that grants nothing is ([`Binding::from_stored`]).
    #[must_use]
    pub fn binding() -> Binding {
        Binding {
            surface: Self::SURFACE.to_string(),
            hash: None,
            scope: Value::Null,
            operations: Self::definition()
                .operations()
                .map(|o| o.name)
                .filter(|name| *name != Self::HIDDEN)
                .map(str::to_string)
                .collect(),
        }
    }

    /// The state, locked.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, FakeSurfaceState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether `hook` is to fail now: once for each time a test asked.
    fn fails(&self, hook: Hook) -> clax_core::Result<()> {
        let mut st = self.lock();
        match st.fail_hooks.iter().position(|h| *h == hook) {
            Some(at) => {
                st.fail_hooks.remove(at);
                Err(clax_core::CoreError::invalid(
                    "broken",
                    format!("the fake's {hook:?} hook is broken, once"),
                ))
            }
            None => Ok(()),
        }
    }

    fn hook(&self, hook: Hook, run: &str, step: Option<(i64, i64)>, end: Option<&End>) {
        self.hook_with(hook, run, step, end, None);
    }

    fn hook_with(
        &self,
        hook: Hook,
        run: &str,
        step: Option<(i64, i64)>,
        end: Option<&End>,
        result: Option<&Value>,
    ) {
        let status = self.store.run(run).ok().flatten().map(|r| r.status);
        self.lock().hooks.push(FakeHook {
            hook,
            run: run.to_string(),
            step,
            status,
            end: end.cloned(),
            result: result.cloned(),
        });
    }

    /// Applies the write `op` on `args` under `key` once: a repeat of
    /// the key answers what the first answered and changes nothing.
    fn apply(
        st: &mut FakeSurfaceState,
        key: &str,
        op: &str,
        args: &[Value],
        write: impl FnOnce(&mut FakeSurfaceState) -> Value,
    ) -> Result<Value, HostError> {
        if let Some(a) = st.applied.iter().find(|a| a.key == key) {
            return if a.op == op && a.args == args {
                Ok(a.answer.clone())
            } else {
                Err(HostError::new(
                    "key_reused",
                    format!("key {key} was used for {} before", a.op),
                ))
            };
        }
        let answer = write(st);
        st.applied.push(FakeApplied {
            key: key.to_string(),
            op: op.to_string(),
            args: args.to_vec(),
            answer: answer.clone(),
        });
        Ok(answer)
    }

    /// Whether the hooks received for run `run` are what the run loop
    /// owes a provider: every `ending` while the run had not ended, at
    /// one step, once for each attempt; then `ended`, at least once,
    /// each time when the run had ended and with an end of the same
    /// kind (its details may differ: [`SurfaceProvider::ended`]); and
    /// no other hook after the first `ended`.
    ///
    /// # Errors
    /// What was otherwise, in words.
    pub fn lifecycle(&self, run: &str) -> Result<(), String> {
        let st = self.lock();
        let hooks: Vec<&FakeHook> = st.hooks.iter().filter(|h| h.run == run).collect();
        let Some(at) = hooks.iter().position(|h| h.hook == Hook::Ended) else {
            return Err("`ended` was called 0 times".into());
        };
        if let Some(later) = hooks[at..].iter().find(|h| h.hook != Hook::Ended) {
            return Err(format!("{:?} was called after `ended`", later.hook));
        }
        let last = hooks[at];
        for repeat in &hooks[at..] {
            if repeat.end.as_ref().map(End::status) != last.end.as_ref().map(End::status) {
                return Err(format!(
                    "`ended` was told {:?}, and {:?} when it was called again",
                    last.end, repeat.end
                ));
            }
            if !repeat.status.is_some_and(RunStatus::is_terminal) {
                return Err(format!(
                    "`ended` was called when the run was {:?}: before its terminal commit",
                    repeat.status
                ));
            }
        }
        let endings: Vec<&&FakeHook> = hooks.iter().filter(|h| h.hook == Hook::Ending).collect();
        let Some(first) = endings.first() else {
            return Err("`ended` was called without `ending`".into());
        };
        let mut attempts = Vec::new();
        for e in &endings {
            if e.status.is_none_or(RunStatus::is_terminal) {
                return Err(format!(
                    "`ending` was called when the run was {:?}: not inside its terminal step",
                    e.status
                ));
            }
            if e.end.as_ref().map(End::status) != last.end.as_ref().map(End::status) {
                return Err(format!(
                    "`ending` was told {:?} and `ended` {:?}",
                    e.end, last.end
                ));
            }
            let (step, attempt) = e.step.unwrap_or_default();
            if Some(step) != first.step.map(|s| s.0) {
                return Err(format!(
                    "`ending` was called at steps {:?} and {step}",
                    first.step
                ));
            }
            if attempts.contains(&attempt) {
                return Err(format!(
                    "`ending` was called twice in attempt {attempt} of step {step}"
                ));
            }
            attempts.push(attempt);
        }
        Ok(())
    }
}

/// A [`FakeSurfaceProvider`]'s session: the names its step set.
struct FakeSession<'s> {
    provider: &'s FakeSurfaceProvider,
    run: String,
    place: (i64, i64),
    set: Mutex<Vec<String>>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl StepSession for FakeSession<'_> {
    async fn call(&self, call: Call<'_>) -> Answer {
        let name = call.args.first().and_then(Value::as_str).unwrap_or("");
        let target = (!name.is_empty()).then(|| name.to_string());
        let result = {
            let mut st = self.provider.lock();
            st.calls.push(FakeCall {
                key: call.place.key.clone(),
                op: call.op.to_string(),
                args: call.args.to_vec(),
                replayed: call.replay.is_some(),
            });
            if let Some(entry) = call.replay {
                recorded(entry)
            } else if let Some(e) = st.unwritten_next.remove(call.op) {
                return Answer::unwritten(e).naming(target);
            } else if let Some(e) = st.fail_next.remove(call.op) {
                Err(e)
            } else if let Some(at) = st.wait_next.iter().position(|op| op == call.op) {
                st.wait_next.remove(at);
                return Answer::waiting().naming(target);
            } else {
                match call.op {
                    "fake.count" => {
                        st.count += 1;
                        Ok(json!(st.count))
                    }
                    "fake.get" => Ok(st.values.get(name).map_or(Value::Null, |v| json!(v))),
                    "fake.set" => {
                        let value = call.args.get(1).and_then(Value::as_str).unwrap_or("");
                        FakeSurfaceProvider::apply(
                            &mut st,
                            &call.place.key,
                            call.op,
                            call.args,
                            |st| {
                                st.values.insert(name.to_string(), value.to_string());
                                json!({"name": name, "writes": st.applied.len() + 1})
                            },
                        )
                    }
                    "fake.ask" => {
                        // The question is kept once for the key, asked
                        // or asked again; its answer is the test's to
                        // supply.
                        let kept = FakeSurfaceProvider::apply(
                            &mut st,
                            &call.place.key,
                            call.op,
                            call.args,
                            |_| Value::Null,
                        );
                        match (kept, st.answers.get(name)) {
                            (Err(e), _) => Err(e),
                            (Ok(_), Some(answer)) => Ok(json!(answer)),
                            (Ok(_), None) => return Answer::waiting().naming(target),
                        }
                    }
                    "fake.fail" => Err(HostError::new("broken", "the fake is broken").retryable()),
                    "fake.refuse" => Err(HostError::new(
                        "forbidden",
                        format!("the fake does not keep {name:?}"),
                    )),
                    other => Err(HostError::new(
                        CODE_UNKNOWN_OP,
                        format!("clax.{other} is not an operation the fake performs"),
                    )),
                }
            }
        };
        if result.is_ok() && call.op == "fake.set" {
            self.set
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(name.to_string());
        }
        Answer::of(result).naming(target)
    }

    /// The names the step set, whichever way it ends: the fake stages
    /// nothing for the step's commit.
    fn finish(self: Box<Self>, _end: StepEnd) -> StepParts {
        self.provider
            .hook(Hook::Finish, &self.run, Some(self.place), None);
        let set = self.set.into_inner().unwrap_or_else(|e| e.into_inner());
        let mut payload = Fields::new();
        payload.insert("fake_set".into(), json!(set));
        StepParts {
            payload,
            ..StepParts::default()
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SurfaceProvider for FakeSurfaceProvider {
    fn definition(&self) -> &Definition {
        &FAKE_DEFINITION
    }

    /// Every operation but [`FakeSurfaceProvider::HIDDEN`], which a run
    /// has only when its binding grants it.
    fn standing(&self, op: &str) -> bool {
        op != Self::HIDDEN
    }

    fn audit_subject(&self, _binding: &Binding) -> Option<AuditIds> {
        self.lock().subject.clone()
    }

    /// The request as stored, as a provider that says nothing answers;
    /// or the error a test asked for (`fail_next`'s `"request"`).
    async fn request(&self, run: &RunInfo) -> Result<Value, HostError> {
        let mut st = self.lock();
        st.requests += 1;
        match st.fail_next.remove("request") {
            Some(e) => Err(e),
            None => Ok(run.request.clone()),
        }
    }

    async fn driver_started(&self) -> clax_core::Result<usize> {
        self.lock().drivers_started += 1;
        Ok(0)
    }

    async fn working(&self, run: &RunInfo) {
        self.hook(Hook::Working, &run.id, None, None);
    }

    async fn begin<'s>(&'s self, cx: StepCx<'s>) -> clax_core::Result<Box<dyn StepSession + 's>> {
        self.hook(Hook::Begin, &cx.run.id, Some((cx.step, cx.attempt)), None);
        self.fails(Hook::Begin)?;
        Ok(Box::new(FakeSession {
            provider: self,
            run: cx.run.id.clone(),
            place: (cx.step, cx.attempt),
            set: Mutex::new(Vec::new()),
        }))
    }

    async fn ending(
        &self,
        cx: StepCx<'_>,
        end: &End,
        result: Option<&Value>,
    ) -> clax_core::Result<Fields> {
        self.hook_with(
            Hook::Ending,
            &cx.run.id,
            Some((cx.step, cx.attempt)),
            Some(end),
            result,
        );
        let text = match (end, result) {
            (End::Finished, Some(r)) => format!("finished: {r}"),
            (End::Finished, None) => "finished".to_string(),
            (End::GaveUp { reason, .. }, _) => format!("gave up: {reason}"),
            (End::Stopped { reason, limit, .. }, _) => match limit {
                Some(limit) => format!("stopped: {reason} at {limit}"),
                None => format!("stopped: {reason}"),
            },
            (End::Failed { code }, _) => format!("failed: {code}"),
        };
        let place = cx.notice(0).ok_or_else(|| {
            clax_core::CoreError::invalid("broken", "the binding has no notice place")
        })?;
        let args = [json!(text)];
        let noticed = FakeSurfaceProvider::apply(&mut self.lock(), &place.key, "notice", &args, {
            let text = text.clone();
            move |_| json!(text)
        });
        // A failure that comes after the write: the attempt made again
        // must not write it twice.
        self.fails(Hook::Ending)?;
        let mut fields = Fields::new();
        fields.insert(
            "fake_notice".into(),
            noticed.unwrap_or_else(|e| json!(e.code)),
        );
        Ok(fields)
    }

    async fn ended(
        &self,
        run: &RunInfo,
        end: &End,
        result: Option<&Value>,
    ) -> Result<(), HostError> {
        self.hook_with(Hook::Ended, &run.id, None, Some(end), result);
        let mut st = self.lock();
        if !st.followed.contains(&run.id) {
            st.followed.push(run.id.clone());
        }
        Ok(())
    }

    /// The runs the fake was told are ending and not yet that they
    /// ended, in the order it first heard of them.
    async fn awaiting_ended(&self) -> clax_core::Result<Vec<String>> {
        let st = self.lock();
        let mut runs: Vec<String> = Vec::new();
        for h in st.hooks.iter().filter(|h| h.hook == Hook::Ending) {
            if !st.followed.contains(&h.run) && !runs.contains(&h.run) {
                runs.push(h.run.clone());
            }
        }
        Ok(runs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_store::bot_runs::Bindings;

    #[test]
    fn a_stored_binding_gives_the_standing_operations_and_the_granted_ones() {
        let standing = |op: &str| !op.starts_with("web.");
        let of = |web: &str| {
            let stored = Bindings::clax("7q3k9mzx2b4t", web);
            Binding::from_stored(
                stored.of(CLAX_SURFACE).expect("the clax binding"),
                crate::api::definition(),
                standing,
            )
        };
        let plain = of("");
        assert_eq!(plain.surface, "clax");
        assert_eq!(plain.hash, None);
        assert_eq!(plain.scope, json!({"artifact": "7q3k9mzx2b4t"}));
        assert_eq!(plain.operations.len(), 22);
        assert!(plain.has("draft.publish") && !plain.has("web.search") && !plain.has("web.fetch"));
        let fetch = of("fetch");
        assert!(fetch.has("web.fetch") && !fetch.has("web.search"));
        let both = of("search,fetch");
        let names: Vec<&str> = crate::api::definition()
            .operations()
            .map(|o| o.name)
            .collect();
        assert_eq!(both.operations, names, "in the file's order");
    }

    #[test]
    fn a_call_is_routed_to_the_surface_it_names_and_else_to_clax() {
        let clax = Binding {
            surface: "clax".into(),
            hash: None,
            scope: Value::Null,
            operations: vec!["files.read".into()],
        };
        let both = [clax, FakeSurfaceProvider::binding()];
        let only_fake = [FakeSurfaceProvider::binding()];
        fn to<'a>(bindings: &'a [Binding], op: &str) -> Option<&'a str> {
            route(bindings, op).map(|b| b.surface.as_str())
        }
        let (both, only_fake) = (&both[..], &only_fake[..]);
        assert_eq!(to(both, "files.read"), Some("clax"));
        assert_eq!(to(both, "fake.set"), Some("fake"));
        // An operation the binding lacks is still its surface's: the
        // refusal is the binding's, not a route to another surface.
        assert_eq!(to(both, "fake.hidden"), Some("fake"));
        assert!(!both[1].has("fake.hidden"));
        assert_eq!(to(both, "nope.op"), Some("clax"));
        assert_eq!(to(both, "fake"), Some("clax"));
        assert_eq!(to(only_fake, "fake.count"), Some("fake"));
        // No binding takes the call: an operation of no surface the
        // run has.
        assert_eq!(to(only_fake, "files.read"), None);
        assert_eq!(to(only_fake, "nope.op"), None);
        assert_eq!(to(&[], "files.read"), None);
    }

    #[test]
    fn arguments_are_checked_against_a_definition_as_claxs_own_are_today() {
        let clax = crate::api::definition();
        for (op, raw) in [
            ("files.read", json!(["index.html"])),
            ("files.read", json!([])),
            ("files.read", json!([7])),
            ("files.write", json!(["a.css", "b", "c"])),
            ("draft.publish", json!([{"note": "n"}])),
            ("draft.publish", json!([null])),
            ("web.search", json!([])),
            ("web.fetch", json!(["https://example.com", {"offset": "x"}])),
            ("threads.reply", json!("not an array")),
            ("nope.op", json!([])),
            ("files", json!([])),
        ] {
            assert_eq!(
                check_args(clax, op, &raw),
                crate::api::check_args(op, &raw),
                "{op} {raw}"
            );
        }
        // A name no definition has, in the words of today's refusal.
        let none = check_args(clax, "nope.op", &json!([])).unwrap_err();
        assert_eq!(none, no_such_operation("nope.op"));
        assert_eq!(
            (none.code.as_str(), none.message.as_str()),
            ("unknown_op", "clax.nope.op is not an operation")
        );
        // Another surface's operations are checked against its own file.
        let fake = FakeSurfaceProvider::definition();
        assert_eq!(
            check_args(fake, "fake.set", &json!(["a", "1"])),
            Ok(vec![json!("a"), json!("1")])
        );
        assert_eq!(
            check_args(fake, "fake.set", &json!(["a"]))
                .unwrap_err()
                .code,
            "invalid_args"
        );
        assert_eq!(
            check_args(fake, "files.read", &json!(["a"])),
            Err(no_such_operation("files.read"))
        );
    }

    fn fields(v: Value) -> Fields {
        v.as_object().cloned().expect("an object")
    }

    #[test]
    fn a_calls_note_adds_what_todays_web_fields_add() {
        let shown = json!({"url": "https://example.com/a", "source": "the request"});
        let base = || json!({"op": "web.fetch", "target": null, "ok": true, "code": null, "ms": 3});
        for (cost, web) in [
            (Some(250), Some(shown.clone())),
            (Some(0), Some(json!({"query": "clax"}))),
            (None, None),
            (Some(7), None),
            (None, Some(Value::Null)),
        ] {
            let mut today = base();
            crate::events::with_web_fields(&mut today, cost, web.as_ref());
            let mut answer = Answer::written(Ok(Value::Null));
            if let Some(cost) = cost {
                answer = answer.charged(cost);
            }
            if let Some(web) = &web {
                answer = answer.showing(fields(json!({"web": web})));
            }
            let mut seam = base();
            answer.note().add_to(&mut seam);
            assert_eq!(seam.to_string(), today.to_string(), "{cost:?} {web:?}");
        }
        // A provider's field never takes one of the core's names.
        let answer = Answer::of(Ok(json!(1)))
            .naming(Some("t".into()))
            .showing(fields(
                json!({"ok": false, "op": "x", "shell": {"window": 2}}),
            ));
        assert_eq!(answer.note().target(), Some("t"));
        assert_eq!(answer.note().cost_milli(), None);
        let mut call = json!({"op": "shell.windows", "ok": true});
        answer.note().add_to(&mut call);
        assert_eq!(
            call,
            json!({"op": "shell.windows", "ok": true, "shell": {"window": 2}})
        );
    }

    #[test]
    fn an_answer_says_who_journals_it_by_its_constructor() {
        let e = || HostError::new("forbidden", "no");
        assert_eq!(Answer::of(Ok(json!(1))).journal(), Journal::Answer);
        assert_eq!(Answer::outcome(Ok(json!(1))).journal(), Journal::Outcome);
        assert_eq!(Answer::refused(e()).journal(), Journal::Outcome);
        assert_eq!(Answer::refused(e()).result(), &Err(e()));
        assert_eq!(Answer::written(Ok(json!(1))).journal(), Journal::Written);
        assert_eq!(Answer::written(Err(e())).into_result(), Err(e()));
        assert_eq!(Answer::of(Ok(json!(1))).note(), &CallNote::default());
        // A failure is the same answer by any of the three the core
        // journals, but for who keeps a value.
        assert_eq!(Answer::outcome(Err(e())), Answer::refused(e()));
        assert_eq!(
            Answer::of(Err(e())).into_result(),
            Answer::refused(e()).into_result()
        );
        // A charge goes with an answer the provider journals; any
        // answer may say it charged nothing.
        let charged = Answer::written(Ok(json!(1))).charged(250);
        assert_eq!(charged.note().cost_milli(), Some(250));
        assert_eq!(
            Answer::refused(e()).uncharged().note().cost_milli(),
            Some(0)
        );
        assert_eq!(Answer::refused(e()).charged(0).note().cost_milli(), Some(0));
        // What `undeclared` answers is a refusal the core journals,
        // whatever made it.
        let other = || HostError::new("unknown_op", "the core's");
        let made = Answer::written(Err(e())).into_refusal(other);
        assert_eq!(
            (made.journal(), made.result()),
            (Journal::Outcome, &Err(e()))
        );
        let made = Answer::written(Ok(json!(1))).into_refusal(other);
        assert_eq!(
            (made.journal(), made.result()),
            (Journal::Outcome, &Err(other()))
        );
        // Nor does what `undeclared` answers wait.
        let made = Answer::waiting().into_refusal(other);
        assert!(!made.is_waiting());
        assert_eq!(
            (made.journal(), made.result()),
            (Journal::Outcome, &Err(other()))
        );
    }

    /// A waiting answer is its own kind: journaled by no one, told from
    /// every other by `is_waiting`, and allowed only to a call of an
    /// operation tagged `@waits` that came with no entry.
    #[test]
    fn only_a_call_of_an_operation_that_waits_and_has_no_entry_may_wait() {
        let waiting = Answer::waiting().naming(Some("q".into()));
        assert!(waiting.is_waiting());
        assert_eq!(waiting.journal(), Journal::Waiting);
        assert_eq!(waiting.result().as_ref().unwrap_err().code, CODE_WAITING);
        for other in [
            Answer::of(Ok(json!(1))),
            Answer::outcome(Ok(json!(1))),
            Answer::refused(HostError::new("x", "y")),
            Answer::written(Ok(json!(1))),
            Answer::unwritten(HostError::new("x", "y")),
        ] {
            assert!(!other.is_waiting(), "{other:?}");
        }
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap());
        let fake = FakeSurfaceProvider::new(store);
        let place = CallPlace::new("r", 1, 1, 0);
        let (args, raw) = (vec![json!("q")], json!(["q"]));
        let call = |op: &'static str, replay| Call {
            place: &place,
            op,
            args: &args,
            raw: &raw,
            replay,
        };
        // Tagged `@waits`, with no entry: as the session answered it.
        let kept = may_wait(waiting.clone(), &fake, &call("fake.ask", None));
        assert_eq!(kept, waiting);
        // Not tagged: refused, for the core to journal, naming what the
        // answer named.
        let refused = may_wait(waiting.clone(), &fake, &call("fake.get", None));
        assert!(!refused.is_waiting());
        assert_eq!(refused.journal(), Journal::Outcome);
        assert_eq!(refused.note().target(), Some("q"));
        let e = refused.result().as_ref().unwrap_err();
        assert_eq!(e.code, CODE_PROVIDER_ERROR);
        assert_eq!(
            e.message,
            "clax.fake.get: its surface answered that the call waits, and it is not tagged @waits"
        );
        // An operation the definition does not have is not tagged.
        let refused = may_wait(waiting.clone(), &fake, &call("fake.nothing", None));
        assert_eq!(
            refused.result().as_ref().unwrap_err().code,
            CODE_PROVIDER_ERROR
        );
        // Any other answer is not looked at.
        let value = Answer::of(Ok(json!(1)));
        assert_eq!(
            may_wait(value.clone(), &fake, &call("fake.get", None)),
            value
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "journaled by its provider")]
    fn a_charge_on_an_answer_the_core_journals_is_refused() {
        let _ = Answer::of(Ok(json!(1))).charged(1);
    }

    #[test]
    fn a_scripts_calls_have_the_places_under_the_notices() {
        let first = CallPlace::in_script("run", 2, 3, 0).expect("the first place");
        assert_eq!(first, CallPlace::new("run", 2, 3, 0));
        assert_eq!(first.key, "run:2:0");
        let last = CallPlace::in_script("run", 2, 3, NOTICE_SEQ - 1).expect("the last place");
        assert_eq!((last.attempt, last.seq), (3, NOTICE_SEQ - 1));
        for outside in [-1, NOTICE_SEQ, NOTICE_SEQ + 1, i64::MAX] {
            assert_eq!(
                CallPlace::in_script("run", 2, 3, outside),
                None,
                "{outside}"
            );
        }
    }

    #[test]
    fn an_unwritten_failure_is_journaled_by_no_one_and_shows_no_cost() {
        let answer = Answer::unwritten(HostError::new("store_failed", "the write failed"));
        assert_eq!(answer.journal(), Journal::Unwritten);
        assert_eq!(answer.note().cost_milli(), None);
        assert_eq!(answer.result().as_ref().unwrap_err().code, "store_failed");
    }

    /// The parts of two sessions joined for one step: `clax`'s fields
    /// at the top of the payload and of what the model is told, each in
    /// its own, another surface's under its name, and the rows in the
    /// order they were joined. Parts that clash are joined in no part.
    #[test]
    fn a_steps_join_keeps_the_payload_apart_from_what_the_model_is_told() {
        let row = |path: &str| DraftChange::Delete { path: path.into() };
        let mut join = StepJoin::new();
        join.add(
            CLAX_SURFACE,
            StepParts {
                payload: fields(json!({"changed": ["a.css"], "published": {"version": 2}})),
                tool_result: fields(json!({"published": {"version": 2}})),
                draft: vec![row("a.css")],
            },
        )
        .unwrap();
        join.add(
            "shell",
            StepParts {
                payload: fields(json!({"windows": 2})),
                tool_result: Fields::new(),
                draft: vec![row("b.css")],
            },
        )
        .unwrap();
        assert_eq!(
            Value::Object(join.told().clone()),
            json!({"published": {"version": 2}})
        );
        let core = fields(json!({"title": "Edit", "calls": []}));
        let payload = join.payload(core.clone());
        assert_eq!(
            payload,
            json!({"title": "Edit", "calls": [], "changed": ["a.css"],
                   "published": {"version": 2}, "shell": {"windows": 2}})
        );
        // A field of what the model is told that is the core's own: the
        // payload half of the same parts is not joined either.
        let before = join.clone();
        let clash = join
            .add(
                "other",
                StepParts {
                    payload: fields(json!({"kept": true})),
                    tool_result: fields(json!({"x": 1})),
                    draft: vec![row("c.css")],
                },
            )
            .and_then(|()| {
                join.add(
                    "value",
                    StepParts {
                        payload: fields(json!({"more": true})),
                        tool_result: fields(json!({"y": 1})),
                        draft: vec![row("d.css")],
                    },
                )
            })
            .unwrap_err();
        assert_eq!((clash.surface.as_str(), clash.core), ("value", true));
        let after_other = {
            let mut j = before.clone();
            j.add(
                "other",
                StepParts {
                    payload: fields(json!({"kept": true})),
                    tool_result: fields(json!({"x": 1})),
                    draft: vec![row("c.css")],
                },
            )
            .unwrap();
            j
        };
        assert_eq!(join, after_other, "the parts that clashed left nothing");
        // A payload field that is the core's own.
        let before = join.clone();
        let clash = join
            .add(
                CLAX_SURFACE,
                StepParts {
                    payload: fields(json!({"calls": 1})),
                    tool_result: fields(json!({"z": 1})),
                    draft: vec![row("e.css")],
                },
            )
            .unwrap_err();
        assert_eq!((clash.field.as_str(), clash.core), ("calls", true));
        assert_eq!(join, before);
        let commit = join.commit(payload.clone(), Vec::new(), Vec::new());
        assert_eq!(commit.payload, payload);
        assert_eq!(commit.draft, [row("a.css"), row("b.css"), row("c.css")]);
        assert_eq!(commit.reason, None);
    }

    #[test]
    fn claxs_fields_are_merged_at_the_top_and_another_surfaces_under_its_name() {
        // A script step's payload as `script.rs` builds it: the core's
        // fields, then `clax`'s under their own names.
        let core = json!({"tool_use_id": "tu_1", "title": "Edit", "code": "return 1;",
                          "outcome": {"value": 1}, "result": {"type": "tool_result"},
                          "calls": []});
        let clax = json!({"changed": ["index.html"], "replied": [], "published": null,
                          "created": false, "base": null});
        let mut payload = fields(core.clone());
        let mut merged = Merged::new();
        let keys = &PAYLOAD_KEYS;
        merge_fields(
            &mut payload,
            &mut merged,
            keys,
            "clax",
            fields(clax.clone()),
        )
        .unwrap();
        let mut today = fields(core);
        today.extend(fields(clax));
        assert_eq!(
            Value::Object(payload.clone()).to_string(),
            Value::Object(today).to_string(),
            "today's bytes"
        );
        assert_eq!(merged.by("changed"), Some("clax"));
        assert_eq!(merged.by("calls"), None, "the core's own");
        // Another surface's fields are one object, under its name.
        let first = fields(json!({"changed": ["window 2"], "calls": 3}));
        merge_fields(&mut payload, &mut merged, keys, "shell", first).unwrap();
        assert_eq!(merged.by("shell"), Some("shell"));
        // What the same surface adds later to the step joins it.
        let later = fields(json!({"notice": "closed"}));
        merge_fields(&mut payload, &mut merged, keys, "shell", later).unwrap();
        assert_eq!(
            payload["shell"],
            json!({"changed": ["window 2"], "calls": 3, "notice": "closed"})
        );
        assert_eq!(payload["changed"], json!(["index.html"]));
        assert_eq!(payload["calls"], json!([]));
        // No fields, no key.
        merge_fields(&mut payload, &mut merged, keys, "fake", Fields::new()).unwrap();
        assert!(!payload.contains_key("fake"));
        assert_eq!(merged.by("fake"), None);
    }

    /// The clash `merge_fields` answers, with what it merged into left
    /// as it was.
    fn clash_of(
        payload: &mut Fields,
        merged: &mut Merged,
        surface: &str,
        answered: Value,
    ) -> (String, bool) {
        let before = (payload.clone(), merged.clone());
        let clash = merge_fields(payload, merged, &PAYLOAD_KEYS, surface, fields(answered))
            .expect_err("a clash");
        assert_eq!(clash.surface, surface);
        assert_eq!(
            (&*payload, &*merged),
            (&before.0, &before.1),
            "a refused merge changes nothing"
        );
        (clash.field, clash.core)
    }

    #[test]
    fn a_field_that_takes_a_name_of_the_cores_or_one_taken_is_the_providers_error() {
        let mut payload = fields(json!({"reason": "run_cap"}));
        let mut merged = Merged::new();
        let (p, m) = (&mut payload, &mut merged);
        merge_fields(
            p,
            m,
            &PAYLOAD_KEYS,
            "clax",
            fields(json!({"notice": "I stopped."})),
        )
        .unwrap();
        // `clax` answering a core name, though the payload has no such
        // field yet.
        let clash = merge_fields(
            p,
            m,
            &PAYLOAD_KEYS,
            "clax",
            fields(json!({"posted": true, "calls": []})),
        )
        .unwrap_err();
        assert_eq!(
            clash,
            FieldClash {
                surface: "clax".into(),
                field: "calls".into(),
                core: true
            }
        );
        assert!(clash.to_string().contains("calls"), "{clash}");
        assert!(!p.contains_key("posted"), "none of the fields went in");
        // `clax` answering one of its own fields a second time.
        assert_eq!(
            clash_of(p, m, "clax", json!({"notice": "again"})),
            ("notice".into(), false)
        );
        // A core name, there already.
        assert_eq!(
            clash_of(p, m, "clax", json!({"reason": "mine"})),
            ("reason".into(), true)
        );
        // A surface whose name is a core field, or a field `clax` has.
        assert_eq!(
            clash_of(p, m, "result", json!({"x": 1})),
            ("result".into(), true)
        );
        assert_eq!(
            clash_of(p, m, "notice", json!({"x": 1})),
            ("notice".into(), false)
        );
        // Another surface answering one of its own fields twice.
        merge_fields(
            p,
            m,
            &PAYLOAD_KEYS,
            "shell",
            fields(json!({"notice": "closed"})),
        )
        .unwrap();
        assert_eq!(
            clash_of(p, m, "shell", json!({"more": 1, "notice": "again"})),
            ("shell.notice".into(), false)
        );
        // What a model is told has its own names.
        let mut told = fields(json!({"value": 1, "logs": [], "calls": []}));
        let mut merged = Merged::new();
        merge_fields(
            &mut told,
            &mut merged,
            &TOOL_RESULT_KEYS,
            "clax",
            fields(json!({"published": {"version": 2}})),
        )
        .unwrap();
        assert_eq!(told["published"], json!({"version": 2}));
        let clash = merge_fields(
            &mut told,
            &mut merged,
            &TOOL_RESULT_KEYS,
            "clax",
            fields(json!({"replay": {}})),
        )
        .unwrap_err();
        assert!(clash.core);
    }

    /// `clax`'s `published` is an object, as a surface's own object is:
    /// a surface named `published` is not merged into it, whichever of
    /// the two is merged first.
    #[test]
    fn a_surface_named_as_a_field_of_claxs_clashes_whichever_is_merged_first() {
        let clax = || fields(json!({"published": {"version": 2}, "changed": []}));
        let host = || fields(json!({"x": 1}));
        // The kind's surface first.
        let mut payload = Fields::new();
        let mut merged = Merged::new();
        merge_fields(&mut payload, &mut merged, &PAYLOAD_KEYS, "clax", clax()).unwrap();
        assert_eq!(
            clash_of(&mut payload, &mut merged, "published", json!({"x": 1})),
            ("published".into(), false)
        );
        assert_eq!(
            payload["published"],
            json!({"version": 2}),
            "not merged into"
        );
        // The host's surface first: the clash is `clax`'s then, for the
        // same name.
        let mut payload = Fields::new();
        let mut merged = Merged::new();
        merge_fields(
            &mut payload,
            &mut merged,
            &PAYLOAD_KEYS,
            "published",
            host(),
        )
        .unwrap();
        assert_eq!(
            clash_of(
                &mut payload,
                &mut merged,
                "clax",
                json!({"published": {"version": 2}, "changed": []})
            ),
            ("published".into(), false)
        );
        assert_eq!(payload, fields(json!({"published": {"x": 1}})));
        // An object the core put under the surface's name is not the
        // surface's either.
        let mut payload = fields(json!({"shell": {"core": true}}));
        let mut merged = Merged::new();
        assert_eq!(
            clash_of(&mut payload, &mut merged, "shell", json!({"x": 1})),
            ("shell".into(), false)
        );
    }

    #[test]
    fn a_charge_the_core_would_journal_is_dropped_from_the_note() {
        let charged = |journal: Journal, cost: Option<i64>| Answer {
            result: Ok(json!(1)),
            journal,
            note: CallNote {
                target: Some("t".into()),
                cost_milli: cost,
                extra: fields(json!({"web": {"query": "q"}})),
            },
            deadline: None,
        };
        let cost = |a: Answer| {
            let a = without_a_charge_the_core_would_journal(a, "web.search");
            assert_eq!(a.note().target(), Some("t"), "the rest of the note stays");
            assert_eq!(a.note().extra().len(), 1);
            a.note().cost_milli()
        };
        // What the release build does where a debug build asserts.
        assert_eq!(cost(charged(Journal::Answer, Some(250))), None);
        assert_eq!(cost(charged(Journal::Outcome, Some(1))), None);
        // A charge the provider journaled, and no charge, stay.
        assert_eq!(cost(charged(Journal::Written, Some(250))), Some(250));
        assert_eq!(cost(charged(Journal::Outcome, Some(0))), Some(0));
        assert_eq!(cost(charged(Journal::Answer, None)), None);
    }

    #[test]
    fn each_binding_has_its_own_places_for_what_its_ending_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        let driver = DriverId::mint();
        let cancel = CancelToken::new();
        let binding = |surface: &str| Binding {
            surface: surface.into(),
            hash: None,
            scope: Value::Null,
            operations: Vec::new(),
        };
        let run = |kind: &str, surfaces: &[&str]| RunInfo {
            id: "01RUN".into(),
            person: "*owner".into(),
            model: "m".into(),
            kind: kind.into(),
            request: Value::Null,
            bindings: surfaces.iter().map(|s| binding(s)).collect(),
        };
        let first = |run: &RunInfo, surface: &str| {
            let b = binding(surface);
            let claim = Claim::new(&store, &driver, &run.id);
            let cx = StepCx::new(run, &b, 4, 2, &[], claim, &cancel);
            (
                cx.notice(0).expect("a place").seq,
                cx.notice(NOTICE_STRIDE - 1).expect("a place").seq,
                cx.notice(NOTICE_STRIDE),
                cx.notice(-1),
            )
        };
        // One binding, the kind's: today's places.
        let alone = run("clax.thread", &["clax"]);
        let claim = Claim::new(&store, &driver, &alone.id);
        let cx = StepCx::new(&alone, &alone.bindings[0], 4, 2, &[], claim, &cancel);
        let place = cx.notice(3).unwrap();
        assert_eq!(
            (place.seq, place.key.as_str()),
            (NOTICE_SEQ + 3, "01RUN:4:1000003")
        );
        assert_eq!((place.step, place.attempt), (4, 2));
        // A script's calls have the places under the first notice's,
        // and no other.
        assert_eq!(cx.place(0).map(|p| p.seq), Some(0));
        let last = cx.place(NOTICE_SEQ - 1).expect("the last place");
        assert_eq!(last.key, "01RUN:4:999999");
        assert_eq!((cx.place(NOTICE_SEQ), cx.place(-1)), (None, None));
        let refusal = no_place(NOTICE_SEQ);
        assert_eq!(refusal.code, CODE_NO_PLACE);
        assert!(refusal.message.contains("1000000"), "{}", refusal.message);
        assert_eq!(
            first(&alone, "clax"),
            (NOTICE_SEQ, NOTICE_SEQ + NOTICE_STRIDE - 1, None, None)
        );
        // Several: the kind's surface first, wherever it was bound, the
        // others above it in the order they were bound, no range
        // meeting another.
        let several = run("review.job", &["clax", "review", "shell"]);
        let starts: Vec<i64> = ["review", "clax", "shell", "unbound"]
            .iter()
            .map(|s| first(&several, s).0)
            .collect();
        assert_eq!(
            starts,
            [
                NOTICE_SEQ,
                NOTICE_SEQ + NOTICE_STRIDE,
                NOTICE_SEQ + 2 * NOTICE_STRIDE,
                NOTICE_SEQ + 3 * NOTICE_STRIDE
            ]
        );
        assert_eq!(
            first(&several, "clax").1,
            NOTICE_SEQ + 2 * NOTICE_STRIDE - 1
        );
    }

    #[test]
    fn the_fakes_file_is_a_hosts_surface_and_its_binding_leaves_one_operation_out() {
        let fake = FakeSurfaceProvider::definition();
        assert_eq!(fake.name(), FakeSurfaceProvider::SURFACE);
        let names: Vec<&str> = fake.operations().map(|o| o.name).collect();
        assert_eq!(
            names,
            [
                "fake.count",
                "fake.get",
                "fake.set",
                "fake.fail",
                "fake.refuse",
                "fake.ask",
                "fake.hidden"
            ]
        );
        let binding = FakeSurfaceProvider::binding();
        assert_eq!(binding.operations, names[..6]);
        let ask = fake.operation("fake.ask").unwrap().op;
        assert!(ask.waits && ask.once);
        assert_eq!(
            names
                .iter()
                .filter(|n| fake.operation(n).unwrap().op.waits)
                .count(),
            1
        );
        assert!(!binding.has(FakeSurfaceProvider::HIDDEN));
        assert!(fake.kind("job").is_some());
    }

    #[test]
    fn an_end_says_the_status_the_step_and_the_reason_it_leaves() {
        let cap = End::Stopped {
            reason: "run_cap".into(),
            limit: Some("40 steps".into()),
            credits_reset: None,
        };
        assert_eq!(cap.status(), RunStatus::Stopped);
        assert_eq!(cap.step_kind(), StepKind::Stopped);
        assert_eq!(cap.reason(), Some("run_cap"));
        assert_eq!(End::Finished.reason(), None);
        assert_eq!(
            End::Finished.step_kind().ends_as(),
            Some(RunStatus::Finished)
        );
        let gave_up = End::GaveUp {
            code: "model".into(),
            reason: "I cannot".into(),
        };
        assert_eq!(gave_up.reason(), Some("model"));
        assert_eq!(gave_up.step_kind().ends_as(), Some(gave_up.status()));
        let failed = End::Failed {
            code: "internal_error".into(),
        };
        assert_eq!(failed.step_kind().ends_as(), Some(failed.status()));
        assert_eq!(End::stopped("cancelled").reason(), Some("cancelled"));
    }

    /// The names a host's surface is refused where it is defined
    /// (`clax_surface::STEP_FIELDS`) are the names a surface so named
    /// would clash on here: the run loop's own of a payload and of what
    /// a model is told, and `clax`'s.
    #[test]
    fn the_names_refused_a_hosts_surface_are_the_names_a_step_keeps_fields_under() {
        let here: std::collections::BTreeSet<&str> = PAYLOAD_KEYS
            .iter()
            .chain(&TOOL_RESULT_KEYS)
            .chain(&CLAX_FIELDS)
            .copied()
            .collect();
        let refused: std::collections::BTreeSet<&str> =
            clax_surface::STEP_FIELDS.iter().copied().collect();
        assert_eq!(here, refused);
        assert_eq!(refused.len(), clax_surface::STEP_FIELDS.len());
        // So no such name parses as a host's surface.
        let reserved = Reserved::of(&[crate::api::definition()]);
        for name in &here {
            let host = Author::Host {
                reserved: &reserved,
            };
            let src = format!("declare const {name}: {{}};");
            assert!(
                clax_surface::parse(&src, host, &Limits::default()).is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn a_place_has_the_key_an_op_ctx_has() {
        let place = CallPlace::new("01RUN", 2, 3, 4);
        assert_eq!(place.key, "01RUN:2:4");
        assert_eq!((place.step, place.attempt, place.seq), (2, 3, 4));
    }
}
