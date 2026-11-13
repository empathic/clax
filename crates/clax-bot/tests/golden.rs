//! What a model is sent and what a run leaves behind, pinned byte for byte
//! against the golden copies in `tests/golden/`: the bodies of the model
//! requests of a thread run and a brief run, every step's stored payload,
//! a run's journal, live events and audit records, how a run ends (the
//! notices it posts, on which thread and in what order, and the roster),
//! what a script is answered and what is kept of its calls, and a script
//! run again after a crash.
//!
//! The tests are this module's, by what they pin; their goldens are
//! beside them in `tests/golden/`:
//!
//! - [`names`]: the table of the goldens, which every pin names its
//!   golden through, and the test that compares it with the directory;
//! - [`pins`]: one run's pins ([`pins::Pins`]): what is read back, how
//!   it is normalised and how it is compared;
//! - [`runs`]: a thread run, a brief run and a run that uses the web,
//!   each whole;
//! - [`endings`]: how a run ends (`ending.*.json`);
//! - [`answers`]: what the run loop answers a turn with when it is not a
//!   script's result (`answer.*.json`);
//! - [`scripts`]: what a script's result tells a model (`script.*.json`)
//!   and what is kept of its calls (`calls.*.json`);
//! - [`web`]: the same for the web operations and their refusals;
//! - [`finish`]: the tools a run of a host's kind is sent, whose
//!   `finish` takes the kind's result (`request.fake-verdict.tools.json`);
//! - [`resumed`]: a script run again after a crash (`resumed.*.json`);
//! - [`cancelled`]: a script that is cancelled
//!   (`ending.stopped.cancelled-script.*.json`) and one that fails after
//!   it wrote (`script.error.after-a-write.json`).
//!
//! Most goldens are the pretty-printed JSON of what the test read back: a
//! request body as the fake Messages API parsed it, and rows as the store
//! holds them. Such a golden pins every key, value and array order, and
//! is readable in a diff; it does not pin the order of an object's keys
//! or the spacing of the bytes on the wire. The `request.*.wire.json`
//! goldens do: each is a request body's bytes exactly as they arrived
//! ([`pins::Pins::wire`]), so a change of key order, spacing, escaping or
//! the way a number is written fails them. The order of the keys on the
//! wire is fixed today because the body is serialised from a
//! `serde_json::Value` built without serde_json's `preserve_order`
//! feature, which writes every object's keys sorted; a body that stops
//! being a `Value` (a struct writes its fields in declaration order), or
//! a build that turns that feature on, changes the bytes and fails the
//! wire goldens while the readable ones still pass.
//!
//! A golden is rewritten with
//! `CLAX_UPDATE_GOLDEN=1 bazel run //crates/clax-bot:integration` (the
//! helper, `src/golden.rs`, says more), and its diff is reviewed with the
//! change that caused it. A new golden is named in [`names`] first.
//!
//! What is normalised, and nothing else ([`pins`]):
//!
//! - The IDs minted for each test's own home, which differ on every run:
//!   the artifact's ID, the IDs of its two threads and the run's ID are
//!   replaced by `<artifact>`, `<thread-1>`, `<thread-2>` and `<run>`
//!   wherever they appear, and a comment's ID (`comment_id`: of a reply
//!   a script posted, or of the sender's comment a fetched URL came
//!   from) by `<comment>`.
//! - `code_sha256`, the hash of a script's code, which names a thread by
//!   its ID.
//! - What a script's run measured, which differs with the machine's load:
//!   `ms`, `us`, `busy_us`, `cpu_us`, `wall_us`, `cpu_ms`, `mem_peak` and
//!   `mem_peak_bytes`; and the credits charged from them: the
//!   `credits_milli` of a `bot.script` audit record and of the
//!   `bot.run_end` record of a run that ran a script
//!   ([`pins::Pins::audit`]), and in the live events the `credits_milli`
//!   of a script's `step_done` and what the run had been charged from
//!   then on ([`pins::Pins::events`]).
//! - `clax_version` and `clax_commit` in an audit record's envelope, which
//!   are the build's.
//! - In a `script.*.json` golden alone, a string over 200 bytes is given
//!   by its length and its first characters ([`scripts::abbreviated`]):
//!   those scripts make large results on purpose.
//! - The roster: how often ClaxBot was shown as working is not pinned,
//!   only that it was before each step and when it stopped being
//!   ([`pins::Pins::ending`]).
//!
//! Each is replaced by a name in angle brackets, and only where its key
//! is present with a value: a key that goes missing or turns null fails
//! its golden. A wire golden has only the four IDs replaced.
//! Times are not normalised: the store's clock is the tests' own
//! ([`crate::common::Env::clock`]), so they are the same on every run.
//! What a web search and a web fetch cost is set by the tests that make
//! them ([`pins::price_the_web`]); what a model's tokens cost is the
//! store's.
//!
//! What is held once: a request after a run's first has its `system` and
//! `tools` replaced by a note once the test has found them equal to the
//! first request's ([`pins::after_the_first`]), and the `system` a run's
//! first turn stores likewise ([`pins::Pins::steps`]); the wire goldens
//! are of a run's first and last request.

mod answers;
mod cancelled;
mod endings;
mod finish;
mod names;
mod pins;
mod resumed;
mod runs;
mod scripts;
mod web;
