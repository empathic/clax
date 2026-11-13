//! What a script's result tells a model (`script.*.json`): what each
//! operation the run performs itself returns, each refusal, the sandbox's
//! own failures, results cut to fit and a replay that diverged. For the
//! runs whose scripts make the calls no other golden holds, what is kept
//! of the calls too (`calls.*.json`, [`Pins::script_calls`]): the step's
//! `calls` and its journal rows.

use super::names::{self as g, Golden};
use super::pins::{ATTEMPT, Pins, driven};
use crate::common::*;
use clax_bot::Next;
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, RequestKind, RunStatus, StepKind};
use serde_json::{Value, json};

/// Strings over 200 bytes in `v`, each replaced by its length and its
/// first 40 characters: a fixture of a result that is large on purpose
/// stays readable, and still pins how large each part is.
pub fn abbreviated(v: &Value) -> Value {
    match v {
        Value::String(s) if s.len() > 200 => {
            let head: String = s.chars().take(40).collect();
            json!(format!("<{} bytes, starting {head:?}>", s.len()))
        }
        Value::Array(a) => a.iter().map(abbreviated).collect(),
        Value::Object(o) => o
            .iter()
            .map(|(k, x)| (k.clone(), abbreviated(x)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        other => other.clone(),
    }
}

/// The `tool_result` of the script step `n` of the run, as stored.
fn tool_result(p: &Pins, n: usize) -> Value {
    let (_, steps) = p.env.store.run_with_steps(&p.run).unwrap().unwrap();
    let step = &steps[n - 1];
    assert_eq!(step.kind, StepKind::Script);
    step.payload.as_ref().unwrap()["result"].clone()
}

/// The `tool_result` of the script step `n` of the run, for a fixture:
/// whether it is an error, and its content, which is JSON text, parsed
/// (or as text when it is not JSON: a result cut to fit), with long
/// strings abbreviated ([`abbreviated`]).
pub fn script_result(p: &Pins, n: usize) -> Value {
    let result = tool_result(p, n);
    let text = result["content"].as_str().unwrap();
    let content = serde_json::from_str::<Value>(text).unwrap_or_else(|_| json!(text));
    json!({
        "is_error": result.get("is_error"),
        "content_bytes": text.len(),
        "content": abbreviated(&content),
    })
}

/// A thread run of one script (`code`) and a finish, prepared by `setup`,
/// driven to its end. The script is step 2.
async fn script_run(code: &str, setup: impl FnOnce(&Env)) -> Pins {
    let env = Env::new().await;
    setup(&env);
    env.api.push(env.script_turn("tu_1", "Script", code));
    env.api.push(finish_turn("done"));
    let run = env.run();
    driven(env, run).await
}

/// What `golden` is, to a reader of a failure.
fn what(golden: Golden) -> String {
    format!("what a script's result tells a model ({})", golden.file())
}

/// What a `calls.*.json` golden is, to a reader of a failure.
fn what_calls(golden: Golden) -> String {
    format!(
        "what is kept of a script's calls ({}): the step's `calls` and its journal rows",
        golden.file()
    )
}

/// Pins the result of the script of [`script_run`] as `golden`.
async fn pin_script(golden: Golden, code: &str, setup: impl FnOnce(&Env)) {
    let p = script_run(code, setup).await;
    p.pin(&what(golden), golden, &script_result(&p, 2));
}

/// The draft operations, which the run performs itself: what each returns
/// and each refusal a script can meet (wrong arguments, the run's rules,
/// the draft's own); and what is kept of each call: its target, and its
/// journal row, which a refusal has too.
#[tokio::test]
async fn the_draft_operations_answers_are_pinned() {
    let code = format!(
        r##"{ATTEMPT}
await attempt("files.list", () => clax.files.list());
await attempt("files.read (its length)", async () => (await clax.files.read("index.html")).length);
await attempt("files.read: no argument", () => clax.files.read());
await attempt("files.read: not a string", () => clax.files.read(1));
await attempt("files.read: too many arguments", () => clax.files.read("a", "b"));
await attempt("files.read: no such file", () => clax.files.read("nope.html"));
await attempt("files.write", () => clax.files.write("extra.css", "h1 {{ color: blue; }}"));
await attempt("files.write: too many arguments", () => clax.files.write("a", "b", "c"));
await attempt("files.replace", () => clax.files.replace("index.html", "--accent: #c00;", "--accent: #00c;"));
await attempt("files.replace: empty old text", () => clax.files.replace("index.html", "", "x"));
await attempt("files.replace: no match", () => clax.files.replace("index.html", "no such text", "x"));
await attempt("files.replace: several matches", () => clax.files.replace("index.html", "<", "x"));
await attempt("files.replace: no such file", () => clax.files.replace("nope.html", "a", "b"));
await attempt("files.delete", () => clax.files.delete("extra.css"));
await attempt("files.delete: no such file", () => clax.files.delete("nope.css"));
await attempt("draft.publish: not an object", () => clax.draft.publish("x"));
await attempt("draft.publish: an unknown field", () => clax.draft.publish({{nope: 1}}));
await attempt("draft.publish: note not a string", () => clax.draft.publish({{note: 1}}));
await attempt("draft.publish: addresses not an array", () => clax.draft.publish({{addresses: "T1"}}));
await attempt("draft.publish: an address not a string", () => clax.draft.publish({{addresses: ["T1", 2]}}));
await attempt("threads.read: a thread that is not the run's", () => clax.threads.read("T2"));
await attempt("threads.reply: a thread that is not the run's", () => clax.threads.reply("T2", "x"));
await attempt("artifacts.create: in a thread request", () => clax.artifacts.create({{title: "New"}}));
await attempt("draft.check: a draft that passes", () => clax.draft.check());
await clax.files.write("bad.html", "<html><head><script>let = ;</script><link rel=\"stylesheet\" href=\"/abs.css\"></head><body><img src=\"missing.png\"></body></html>");
await attempt("draft.check: a page that breaks the contract", () => clax.draft.check());
await clax.files.delete("bad.html");
await attempt("draft.publish", () => clax.draft.publish({{note: "Blue heading", addresses: ["T1"]}}));
await attempt("draft.publish: twice in one script", () => clax.draft.publish({{note: "again"}}));
return out;"##
    );
    let p = script_run(&code, |_| {}).await;
    p.pin_all([
        p.json(
            &what(g::SCRIPT_DRAFT_OPERATIONS),
            g::SCRIPT_DRAFT_OPERATIONS,
            &script_result(&p, 2),
        ),
        p.json(
            &what_calls(g::CALLS_DRAFT_OPERATIONS),
            g::CALLS_DRAFT_OPERATIONS,
            &p.script_calls(),
        ),
    ]);
}

// The sandbox's own failures, as QuickJS words them: a syntax error, a
// thrown value that is not an error, too many calls, a result too large.

/// A syntax error.
#[tokio::test]
async fn a_script_that_does_not_parse_is_pinned() {
    pin_script(g::SCRIPT_ERROR_SYNTAX, "const = 1;", |_| {}).await;
}

/// A thrown value that is not an `Error`, after a log line.
#[tokio::test]
async fn a_script_that_throws_a_value_is_pinned() {
    pin_script(
        g::SCRIPT_ERROR_THROWN_VALUE,
        r#"console.log("before"); throw {reason: "not an Error"};"#,
        |_| {},
    )
    .await;
}

/// More calls than the run's settings allow a script; and its audit
/// records: the `bot.script` record lists the calls the script made as
/// the sandbox counts them, the one over the limit among them, which
/// never reached the run loop and has no `bot.call` record.
#[tokio::test]
async fn a_script_that_makes_too_many_calls_is_pinned() {
    let p = script_run(
        "for (let i = 0; i < 5; i++) await clax.files.list(); return 1;",
        |env| env.set(|s| s.script_max_calls = 3),
    )
    .await;
    p.pin_all([
        p.json(
            &what(g::SCRIPT_ERROR_CALL_LIMIT),
            g::SCRIPT_ERROR_CALL_LIMIT,
            &script_result(&p, 2),
        ),
        p.json(
            "the audit records of a script that made too many calls \
             (audit.script.call-limit.json)",
            g::AUDIT_SCRIPT_CALL_LIMIT,
            &p.audit(),
        ),
    ]);
}

/// A call the script made and did not await: the script is over before
/// the call reaches the run loop. The sandbox counts it, so it is in
/// what the model is told and in the `bot.script` record's calls; the
/// run loop never saw it, so it has no `bot.call` record, no journal row
/// and no place in the step's `calls`.
#[tokio::test]
async fn a_call_that_was_not_awaited_is_in_the_scripts_record_alone() {
    let p = script_run(
        r#"await clax.files.list(); clax.files.read("index.html"); return 1;"#,
        |_| {},
    )
    .await;
    p.pin(
        "what is kept of a call a script did not await (audit.script.call-not-awaited.json): \
         the script's result, the step's calls and journal, and the audit records",
        g::AUDIT_SCRIPT_CALL_NOT_AWAITED,
        &json!({
            "audit": p.audit(),
            "calls": p.script_calls(),
            "result": script_result(&p, 2),
        }),
    );
}

/// A returned value too large to be a result.
#[tokio::test]
async fn a_script_that_returns_too_much_is_pinned() {
    pin_script(
        g::SCRIPT_ERROR_RESULT_TOO_LARGE,
        r#"return "x".repeat(70 * 1024);"#,
        |_| {},
    )
    .await;
}

/// Logs that do not fit the result are halved until it fits, with a line
/// saying so.
#[tokio::test]
async fn logs_that_do_not_fit_are_cut() {
    pin_script(
        g::SCRIPT_LOGS_CUT_TO_FIT,
        r#"for (let i = 0; i < 40; i++) console.log(String(i).padStart(2, "0") + "y".repeat(4000));
return "x".repeat(60 * 1024);"#,
        |_| {},
    )
    .await;
}

/// A result that does not fit even without logs is cut, and says so: it
/// is then not JSON, and is pinned by its length, its start and its end.
#[tokio::test]
async fn a_result_that_does_not_fit_is_cut() {
    let p = script_run(r#"throw new Error("x".repeat(120 * 1024));"#, |_| {}).await;
    let result = tool_result(&p, 2);
    let text = result["content"].as_str().unwrap();
    let chars: Vec<char> = text.chars().collect();
    p.pin(
        &what(g::SCRIPT_RESULT_CUT_TO_FIT),
        g::SCRIPT_RESULT_CUT_TO_FIT,
        &json!({
            "is_error": result.get("is_error"),
            "content_bytes": text.len(),
            "content_starts": chars[..80].iter().collect::<String>(),
            "content_ends": chars[chars.len() - 40..].iter().collect::<String>(),
        }),
    );
}

/// A script run again that makes another call than its journal holds.
#[tokio::test]
async fn a_replay_that_diverged_is_pinned() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Read then reply",
        "const a = await clax.files.list(); await clax.threads.reply('T1', 'hi'); return a.length;",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let deps = env.deps();
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    // The journal says call 0 of step 2 was another operation.
    let entry = env
        .store
        .call_entry(
            &run,
            &d,
            2,
            1,
            0,
            "files.read",
            &clax_bot::host::args_hash(&json!(["index.html"])),
        )
        .unwrap();
    env.store.record_bot_call(&entry).unwrap();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Script));
    let p = Pins::new(env, run);
    p.pin(
        &what(g::SCRIPT_REPLAY_DIVERGED),
        g::SCRIPT_REPLAY_DIVERGED,
        &script_result(&p, 2),
    );
}

/// What a script meets when it creates a second artifact: in the script
/// that created the first, and in a later one; and what is kept of those
/// calls.
#[tokio::test]
async fn a_second_artifact_is_refused() {
    let env = Env::new().await;
    let first = format!(
        r##"{ATTEMPT}
await attempt("artifacts.create (its keys)", async () => Object.keys(await clax.artifacts.create({{title: "New"}})).sort());
await attempt("artifacts.create: a second in the same script", () => clax.artifacts.create({{title: "Again"}}));
return out;"##
    );
    let later = format!(
        r##"{ATTEMPT}
await attempt("artifacts.create: a second in a later script", () => clax.artifacts.create({{title: "Later"}}));
return out;"##
    );
    env.api.push(env.script_turn("tu_1", "Create", &first));
    env.api
        .push(env.script_turn("tu_2", "Create again", &later));
    env.api.push(finish_turn("done"));
    let run = env.run_as(RequestKind::Brief);
    let p = driven(env, run).await;
    p.pin_all([
        p.json(
            &what(g::SCRIPT_ARTIFACT_CREATED_ONCE),
            g::SCRIPT_ARTIFACT_CREATED_ONCE,
            &json!({
                "the script that created one": script_result(&p, 2),
                "a later script": script_result(&p, 4),
            }),
        ),
        p.json(
            &what_calls(g::CALLS_ARTIFACT_CREATED_ONCE),
            g::CALLS_ARTIFACT_CREATED_ONCE,
            &created_ids_named(p.script_calls()),
        ),
    ]);
}

/// `calls` with the ID of the artifact a script created, which is minted
/// by the run and is in its journal row's result alone, replaced by a
/// name where it has a value.
fn created_ids_named(mut calls: Value) -> Value {
    for step in calls.as_array_mut().into_iter().flatten() {
        for row in step["journal"].as_array_mut().into_iter().flatten() {
            if row["op"] == "artifacts.create"
                && let Some(id) = row["result"].get_mut("id")
                && !id.is_null()
            {
                *id = json!("<the created artifact>");
            }
        }
    }
    calls
}

/// A draft over a version someone else published while the run worked:
/// the publish that conflicts, the rebase, what `draft.theirs` reads
/// before and after it, and the publish that follows; and what is kept of
/// those calls: the rebase's journal row holds the version it went to,
/// which a rebase made again goes to.
#[tokio::test]
async fn a_publish_conflict_and_the_rebase_after_it_are_pinned() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Edit",
        r#"await clax.files.replace("index.html", "--accent: #c00;", "--accent: #00c;");
await clax.files.write("extra.css", "h1 { color: blue; }");
return 1;"#,
    ));
    let code = format!(
        r##"{ATTEMPT}
const heading = (html) => html.match(/<h1>.*<\/h1>/)[0];
await attempt("draft.theirs: before a rebase (its heading)", async () => heading(await clax.draft.theirs("index.html")));
await attempt("draft.theirs: no argument", () => clax.draft.theirs());
await attempt("draft.rebase: an argument", () => clax.draft.rebase(2));
await attempt("draft.publish: over a version someone else published", () => clax.draft.publish({{note: "Blue heading"}}));
await attempt("draft.rebase", () => clax.draft.rebase());
await attempt("draft.theirs: a file both changed (its heading)", async () => heading(await clax.draft.theirs("index.html")));
await attempt("draft.theirs: a file theirs does not have", () => clax.draft.theirs("extra.css"));
await attempt("draft.theirs: a file only theirs has", () => clax.draft.theirs("notes.txt"));
await attempt("files.read: the draft keeps its own (its accent and heading)", async () => {{
  const html = await clax.files.read("index.html");
  return [html.match(/--accent: #[0-9a-f]+;/)[0], heading(html)];
}});
await attempt("files.list: after the rebase", () => clax.files.list());
await attempt("draft.rebase: again, with nothing new", () => clax.draft.rebase());
await attempt("draft.publish: after the rebase", () => clax.draft.publish({{note: "Blue heading"}}));
return out;"##
    );
    env.api.push(env.script_turn("tu_2", "Publish", &code));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let deps = env.deps();
    let d = DriverId::mint();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Script));
    someone_publishes(&env, 1);
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let p = Pins::new(env, run);
    p.pin_all([
        p.json(
            &what(g::SCRIPT_DRAFT_REBASE),
            g::SCRIPT_DRAFT_REBASE,
            &script_result(&p, 4),
        ),
        p.json(
            &what_calls(g::CALLS_DRAFT_REBASE),
            g::CALLS_DRAFT_REBASE,
            &p.script_calls(),
        ),
    ]);
}

/// The person publishes the next version of the artifact, over
/// `if_version`, while a run works or before it starts: the page with
/// another heading, and a file of theirs.
pub fn someone_publishes(env: &Env, if_version: u32) {
    let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
        "files": {
            "index.html": {"content": PAGE.replace("Quarterly goals", "NEW goals"), "encoding": "utf8"},
            "notes.txt": {"content": "Theirs.", "encoding": "utf8"}
        },
        "if_version": if_version
    }))
    .unwrap();
    env.store
        .publish_version(
            &AuditCtx::DAEMON,
            &clax_core::ArtifactId::parse(&env.aid).unwrap(),
            clax_core::publish::validate(req).unwrap(),
            None,
        )
        .unwrap();
}
