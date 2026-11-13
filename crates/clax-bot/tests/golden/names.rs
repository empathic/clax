//! The goldens of `tests/golden/`, each named once: a pin takes a
//! [`Golden`], and only this table makes one, so every file a pin reads
//! or rewrites is in it, whichever module the pin is in and however the
//! test is built. [`the_table_names_every_file_of_tests_golden`] compares
//! the table with the directory. A golden of the table that no pin takes
//! is an unused constant: the build warns of it, and the lint
//! (`--config=clippy`, which denies warnings) fails.

use std::collections::BTreeSet;

/// One golden of `tests/golden/`, by its file's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Golden(&'static str);

impl Golden {
    /// The file's name in `tests/golden/`.
    pub fn file(self) -> &'static str {
        self.0
    }

    /// The golden of what an ending's run left beside what this golden,
    /// an ending's, holds (`Pins::ending_net`): the file
    /// `net.<this file>`, which [`NETS`] names.
    ///
    /// # Panics
    /// When [`NETS`] does not name it.
    pub fn net(self) -> Golden {
        let name = format!("net.{}", self.0);
        match NETS.iter().find(|n| **n == name) {
            Some(n) => Golden(n),
            None => panic!("tests/golden/names.rs names no {name}"),
        }
    }

    /// The file's path from the crate's directory.
    pub fn path(self) -> String {
        format!("tests/golden/{}", self.0)
    }
}

/// Declares each golden as a constant, and [`ALL`], every file named.
macro_rules! goldens {
    ($($name:ident = $file:literal;)*) => {
        $(pub const $name: Golden = Golden($file);)*

        /// The name of every golden's file, in the table's order.
        const ALL: &[&str] = &[$($file),*];
    };
}

goldens! {
    ANSWER_MAX_TOKENS_CUT_TOOL_CALL = "answer.max-tokens.cut-tool-call.json";
    ANSWER_MAX_TOKENS_NO_TOOL_CALL = "answer.max-tokens.no-tool-call.json";
    ANSWER_PAUSE_TURN = "answer.pause-turn.json";
    ANSWER_PAUSE_TURN_THEN_NO_TOOL_CALL = "answer.pause-turn.then-no-tool-call.json";
    ANSWER_TOOL_INPUT_FINISH_NOT_ITS_SCHEMA = "answer.tool-input.finish-not-its-schema.json";
    ANSWER_TOOL_INPUT_GIVE_UP_NOT_ITS_SCHEMA = "answer.tool-input.give-up-not-its-schema.json";
    ANSWER_TOOL_INPUT_NOT_ITS_SCHEMA = "answer.tool-input.not-its-schema.json";
    ANSWER_TOOL_INPUT_NOT_JSON = "answer.tool-input.not-json.json";
    ANSWER_TOOL_SEVERAL_CALLS = "answer.tool.several-calls.json";
    ANSWER_TOOL_UNKNOWN = "answer.tool.unknown.json";
    AUDIT_BRIEF = "audit.brief.json";
    AUDIT_FAILED = "audit.failed.json";
    AUDIT_REPLAY_DIVERGED = "audit.replay-diverged.json";
    AUDIT_SCRIPT_CALL_LIMIT = "audit.script.call-limit.json";
    AUDIT_SCRIPT_CALL_NOT_AWAITED = "audit.script.call-not-awaited.json";
    AUDIT_THREAD = "audit.thread.json";
    AUDIT_WEB = "audit.web.json";
    CALLS_ARTIFACT_CREATED_ONCE = "calls.artifact-created-once.json";
    CALLS_DRAFT_OPERATIONS = "calls.draft-operations.json";
    CALLS_DRAFT_REBASE = "calls.draft-rebase.json";
    ENDING_FAILED_INTERNAL_ERROR = "ending.failed.internal-error.json";
    ENDING_FAILED_TWO_THREADS = "ending.failed.two-threads.json";
    ENDING_FINISH_BRIEF = "ending.finish.brief.json";
    ENDING_FINISH_EMPTY_SUMMARY_TWO_THREADS = "ending.finish.empty-summary.two-threads.json";
    ENDING_FINISH_NO_REPLY_SOURCES = "ending.finish.no-reply.sources.json";
    ENDING_FINISH_NO_REPLY_THREE_SOURCES = "ending.finish.no-reply.three-sources.json";
    ENDING_FINISH_NO_REPLY_TWO_THREADS = "ending.finish.no-reply.two-threads.json";
    ENDING_FINISH_NO_REPLY_TWO_THREADS_SOURCES = "ending.finish.no-reply.two-threads.sources.json";
    ENDING_FINISH_REPLIED_AFTER_PUBLISHING = "ending.finish.replied-after-publishing.json";
    ENDING_FINISH_REPLIED_BEFORE_A_DRAFT_WRITE = "ending.finish.replied-before-a-draft-write.json";
    ENDING_FINISH_REPLIED_BEFORE_A_PUBLISH = "ending.finish.replied-before-a-publish.json";
    ENDING_FINISH_REPLIED_ON_ONE_OF_TWO_THREADS = "ending.finish.replied-on-one-of-two-threads.json";
    ENDING_FINISH_REPLIED_ON_ONE_OF_TWO_THREADS_SOURCES = "ending.finish.replied-on-one-of-two-threads.sources.json";
    ENDING_FINISH_REPLIED_SOURCES = "ending.finish.replied.sources.json";
    ENDING_GIVE_UP_AFTER_ONE_SCRIPT_WITHOUT_EDITS = "ending.give-up.after-one-script-without-edits.json";
    ENDING_GIVE_UP_AFTER_TWO_SCRIPTS_WITHOUT_EDITS = "ending.give-up.after-two-scripts-without-edits.json";
    ENDING_GIVE_UP_CONTEXT_WINDOW = "ending.give-up.context-window.json";
    ENDING_GIVE_UP_MODEL = "ending.give-up.model.json";
    ENDING_GIVE_UP_NO_TOOL_CALL = "ending.give-up.no-tool-call.json";
    ENDING_GIVE_UP_REFUSAL = "ending.give-up.refusal.json";
    ENDING_GIVE_UP_TWO_THREADS = "ending.give-up.two-threads.json";
    ENDING_STOPPED_CANCELLED = "ending.stopped.cancelled.json";
    ENDING_STOPPED_CANCELLED_SCRIPT_AFTER_WRITES = "ending.stopped.cancelled-script.after-writes.json";
    ENDING_STOPPED_CANCELLED_SCRIPT_BEFORE_WRITES = "ending.stopped.cancelled-script.before-writes.json";
    ENDING_STOPPED_CANCELLED_SCRIPT_RETURNED = "ending.stopped.cancelled-script.returned.json";
    ENDING_STOPPED_CREDITS_EXHAUSTED_AFTER_PUBLISHING = "ending.stopped.credits-exhausted.after-publishing.json";
    ENDING_STOPPED_CREDITS_EXHAUSTED_BEFORE_THE_FIRST_TURN = "ending.stopped.credits-exhausted.before-the-first-turn.json";
    ENDING_STOPPED_DISABLED = "ending.stopped.disabled.json";
    ENDING_STOPPED_DISABLED_AFTER_A_SCRIPT_THAT_NEVER_COMMITTED = "ending.stopped.disabled.after-a-script-that-never-committed.json";
    ENDING_STOPPED_KEY_REJECTED = "ending.stopped.key-rejected.json";
    ENDING_STOPPED_MODEL_ERROR = "ending.stopped.model-error.json";
    ENDING_STOPPED_MODEL_NOT_PRICED = "ending.stopped.model-not-priced.json";
    ENDING_STOPPED_NO_FLAG = "ending.stopped.no-flag.json";
    ENDING_STOPPED_NOT_CONFIGURED = "ending.stopped.not-configured.json";
    ENDING_STOPPED_REQUEST_GONE = "ending.stopped.request-gone.json";
    ENDING_STOPPED_RUN_CAP_CREDITS = "ending.stopped.run-cap.credits.json";
    ENDING_STOPPED_RUN_CAP_MINUTES = "ending.stopped.run-cap.minutes.json";
    ENDING_STOPPED_RUN_CAP_STEPS = "ending.stopped.run-cap.steps.json";
    ENDING_STOPPED_RUN_CAP_TWO_THREADS = "ending.stopped.run-cap.two-threads.json";
    EVENTS_THREAD = "events.thread.json";
    EVENTS_WEB = "events.web.json";
    JOURNAL_THREAD = "journal.thread.json";
    JOURNAL_WEB = "journal.web.json";
    JOURNAL_WEB_DISABLED = "journal.web-disabled.json";
    REQUEST_BRIEF_1_FIRST = "request.brief.1-first.json";
    REQUEST_BRIEF_1_WIRE = "request.brief.1.wire.json";
    REQUEST_BRIEF_2_AFTER_A_SCRIPT = "request.brief.2-after-a-script.json";
    REQUEST_BRIEF_2_WIRE = "request.brief.2.wire.json";
    REQUEST_FAKE_VERDICT_TOOLS = "request.fake-verdict.tools.json";
    REQUEST_THREAD_1_FIRST = "request.thread.1-first.json";
    REQUEST_THREAD_1_WIRE = "request.thread.1.wire.json";
    REQUEST_THREAD_2_AFTER_A_SCRIPT = "request.thread.2-after-a-script.json";
    REQUEST_THREAD_3_AFTER_A_SCRIPT_ERROR = "request.thread.3-after-a-script-error.json";
    REQUEST_THREAD_4_AFTER_NO_TOOL_CALL = "request.thread.4-after-no-tool-call.json";
    REQUEST_THREAD_4_WIRE = "request.thread.4.wire.json";
    RESUMED_PUBLISH = "resumed.publish.json";
    RESUMED_REBASE_PUBLISH_FETCH = "resumed.rebase-publish-fetch.json";
    RESUMED_SECOND_SCRIPT_AFTER_A_PUBLISH = "resumed.second-script.after-a-publish.json";
    RESUMED_SECOND_SCRIPT_AFTER_A_REBASE = "resumed.second-script.after-a-rebase.json";
    SCRIPT_ARTIFACT_CREATED_ONCE = "script.artifact-created-once.json";
    SCRIPT_DRAFT_OPERATIONS = "script.draft-operations.json";
    SCRIPT_DRAFT_REBASE = "script.draft-rebase.json";
    SCRIPT_ERROR_AFTER_A_WRITE = "script.error.after-a-write.json";
    SCRIPT_ERROR_CALL_LIMIT = "script.error.call-limit.json";
    SCRIPT_ERROR_RESULT_TOO_LARGE = "script.error.result-too-large.json";
    SCRIPT_ERROR_SYNTAX = "script.error.syntax.json";
    SCRIPT_ERROR_THROWN_VALUE = "script.error.thrown-value.json";
    SCRIPT_LOGS_CUT_TO_FIT = "script.logs-cut-to-fit.json";
    SCRIPT_REPLAY_DIVERGED = "script.replay-diverged.json";
    SCRIPT_RESULT_CUT_TO_FIT = "script.result-cut-to-fit.json";
    SCRIPT_WEB_OPERATIONS = "script.web-operations.json";
    SCRIPT_WEB_REFUSALS = "script.web-refusals.json";
    STEPS_BRIEF = "steps.brief.json";
    STEPS_THREAD = "steps.thread.json";
    STEPS_WEB = "steps.web.json";
}

/// What each ending's run left beside its ending golden
/// ([`Golden::net`]): its audit records, usage rows, journal and roster
/// calls. One for every `ending.*.json` an ending's pin takes
/// (`Pins::ending_pinned`); a cancelled script's has its own pins
/// (`cancelled.rs`).
const NETS: &[&str] = &[
    "net.ending.failed.internal-error.json",
    "net.ending.failed.two-threads.json",
    "net.ending.finish.brief.json",
    "net.ending.finish.empty-summary.two-threads.json",
    "net.ending.finish.no-reply.sources.json",
    "net.ending.finish.no-reply.three-sources.json",
    "net.ending.finish.no-reply.two-threads.json",
    "net.ending.finish.no-reply.two-threads.sources.json",
    "net.ending.finish.replied-after-publishing.json",
    "net.ending.finish.replied-before-a-draft-write.json",
    "net.ending.finish.replied-before-a-publish.json",
    "net.ending.finish.replied-on-one-of-two-threads.json",
    "net.ending.finish.replied-on-one-of-two-threads.sources.json",
    "net.ending.finish.replied.sources.json",
    "net.ending.give-up.after-one-script-without-edits.json",
    "net.ending.give-up.after-two-scripts-without-edits.json",
    "net.ending.give-up.context-window.json",
    "net.ending.give-up.model.json",
    "net.ending.give-up.no-tool-call.json",
    "net.ending.give-up.refusal.json",
    "net.ending.give-up.two-threads.json",
    "net.ending.stopped.cancelled.json",
    "net.ending.stopped.credits-exhausted.after-publishing.json",
    "net.ending.stopped.credits-exhausted.before-the-first-turn.json",
    "net.ending.stopped.disabled.json",
    "net.ending.stopped.disabled.after-a-script-that-never-committed.json",
    "net.ending.stopped.key-rejected.json",
    "net.ending.stopped.model-error.json",
    "net.ending.stopped.model-not-priced.json",
    "net.ending.stopped.no-flag.json",
    "net.ending.stopped.not-configured.json",
    "net.ending.stopped.request-gone.json",
    "net.ending.stopped.run-cap.credits.json",
    "net.ending.stopped.run-cap.minutes.json",
    "net.ending.stopped.run-cap.steps.json",
    "net.ending.stopped.run-cap.two-threads.json",
];

/// The table and `tests/golden/` hold the same goldens: a file whose pin
/// was removed or renamed is found, not left behind unread, and a golden
/// named twice is found too. The directory's other files are this test's
/// modules and the directory's annotations (`.qual`). While the goldens are being rewritten (`CLAX_UPDATE_GOLDEN`)
/// a golden of the table may have no file yet: the run is writing it.
#[test]
fn the_table_names_every_file_of_tests_golden() {
    let table: BTreeSet<&str> = ALL.iter().chain(NETS).copied().collect();
    assert_eq!(
        table.len(),
        ALL.len() + NETS.len(),
        "a golden is in the table twice"
    );
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let (modules, files): (Vec<String>, Vec<String>) = std::fs::read_dir(&dir)
        .expect("tests/golden")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|f| f != ".qual")
        .partition(|f| f.ends_with(".rs"));
    assert!(
        !modules.is_empty(),
        "tests/golden holds this test's modules"
    );
    let files: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let unnamed: Vec<&&str> = files.difference(&table).collect();
    let rewriting = std::env::var_os("CLAX_UPDATE_GOLDEN").is_some();
    let missing: Vec<&&str> = table.difference(&files).filter(|_| !rewriting).collect();
    assert!(
        unnamed.is_empty() && missing.is_empty(),
        "tests/golden/ and the table of tests/golden/names.rs differ.\n\
         Files the table does not name (remove them, or name and pin them): {unnamed:?}\n\
         Goldens of the table with no file (remove them, or write them: the header of \
         tests/golden.rs says how): {missing:?}"
    );
}
