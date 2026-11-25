//! Three runs, each pinned whole: a thread run and a brief run (their
//! model requests, stored steps, journal, audit records and ending; the
//! thread run's live events too), and a thread run that uses the web.

use super::names as g;
use super::pins::{
    GIVEN, PUBLISH, Pins, after_the_first, fetched, kinds, price_the_web, sender_says,
};
use super::web::{FOUND, found};
use crate::common::*;
use crate::runs::EDIT;
use clax_bot::host::{ArtifactMeta, RequestContext};
use clax_bot::web::FetchedPage;
use clax_store::bot_runs::{RequestKind, StepKind};

/// A script that logs, makes a call and throws.
const THROWS: &str = r#"console.log("checking the colour");
await clax.files.read("index.html");
throw new Error("no such colour");"#;

/// A thread run of seven steps: a turn that asks for a script, the script
/// ([`EDIT`]: it reads, edits, checks, publishes, replies, logs and
/// returns a value, so its result carries all four parts), a turn that
/// asks for a script that throws, that script, a turn with no tool call
/// (answered by the nudge, which the fourth request ends with), a turn
/// that calls `finish`, and the finish.
#[tokio::test]
async fn a_thread_run_is_pinned() {
    let env = Env::new().await;
    env.api
        .push(env.script_turn("tu_1", "Make the heading blue", EDIT));
    env.api
        .push(env.script_turn("tu_2", "Check the colour", THROWS));
    env.api.push(text_turn("The heading is blue."));
    env.api.push(finish_turn(
        "Made the heading blue and published version 2.",
    ));
    let run = env.run();
    let mut p = Pins::subscribed(env, run);
    assert_eq!(
        kinds(&p.drive().await),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Turn,
            StepKind::Finish
        ]
    );
    let reqs = p.env.api.requests();
    assert_eq!(reqs.len(), 4);
    let first = &reqs[0].body;
    let events = p.events().await;
    p.pin_all([
        p.wire(
            "the first model request of a thread run",
            g::REQUEST_THREAD_1_WIRE,
            &reqs[0],
        ),
        p.wire(
            "the last model request of a thread run, which holds every earlier message",
            g::REQUEST_THREAD_4_WIRE,
            &reqs[3],
        ),
        p.json(
            "the first model request of a thread run (system, messages, tools and every \
             other field of the body)",
            g::REQUEST_THREAD_1_FIRST,
            first,
        ),
        p.json(
            "the second model request of a thread run: the first turn and its script's \
             result (value, logs and calls)",
            g::REQUEST_THREAD_2_AFTER_A_SCRIPT,
            &after_the_first(first, &reqs[1].body),
        ),
        p.json(
            "the third model request of a thread run, after a script that threw (its \
             result's error, logs and calls)",
            g::REQUEST_THREAD_3_AFTER_A_SCRIPT_ERROR,
            &after_the_first(first, &reqs[2].body),
        ),
        p.json(
            "the fourth model request of a thread run, after a turn with no tool call \
             (its last message is the nudge)",
            g::REQUEST_THREAD_4_AFTER_NO_TOOL_CALL,
            &after_the_first(first, &reqs[3].body),
        ),
        p.json(
            "the stored steps of a thread run (turn, script, finish payloads)",
            g::STEPS_THREAD,
            &p.steps(first),
        ),
        p.json(
            "the journal of a thread run (each call's place, operation and outcome)",
            g::JOURNAL_THREAD,
            &p.journal(),
        ),
        p.json(
            "the live events of a thread run (each one's ID, name and data, as a \
             subscriber gets them)",
            g::EVENTS_THREAD,
            &events,
        ),
        p.json(
            "the audit records of a thread run (each step's and call's, with their IDs)",
            g::AUDIT_THREAD,
            &p.audit(),
        ),
        p.ending_pinned(g::ENDING_FINISH_REPLIED_AFTER_PUBLISHING),
        p.ending_net_pinned(g::ENDING_FINISH_REPLIED_AFTER_PUBLISHING),
    ]);
}

/// A brief run of four steps: a turn, a script that publishes the page, a
/// turn that calls `finish`, and the finish, whose summary is posted on
/// the brief's thread (the script did not reply). Its settings name an
/// effort ([`clax_bot::run::RunSettings::effort`]), which every request
/// then carries.
#[tokio::test]
async fn a_brief_run_is_pinned() {
    let env = Env::new().await;
    env.host.lock().context = RequestContext {
        artifact: ArtifactMeta {
            id: env.aid.clone(),
            title: "Untitled".into(),
            description: Some("A page being built".into()),
            version: 1,
            url: Some(format!("https://clax.example/a/{}", env.aid)),
        },
        person_name: Some("Owner".into()),
        threads: Vec::new(),
        brief: Some("A \"status\" page:\none row per service.".into()),
    };
    env.set(|s| s.effort = Some("high".into()));
    env.api
        .push(env.script_turn("tu_1", "Build the page", PUBLISH));
    env.api.push(finish_turn("Built the status page."));
    let run = env.run_as(RequestKind::Brief);
    let p = Pins::new(env, run);
    assert_eq!(
        kinds(&p.drive().await),
        [
            StepKind::Turn,
            StepKind::Script,
            StepKind::Turn,
            StepKind::Finish
        ]
    );
    let reqs = p.env.api.requests();
    assert_eq!(reqs.len(), 2);
    let first = &reqs[0].body;
    p.pin_all([
        p.wire(
            "the first model request of a brief run",
            g::REQUEST_BRIEF_1_WIRE,
            &reqs[0],
        ),
        p.wire(
            "the last model request of a brief run",
            g::REQUEST_BRIEF_2_WIRE,
            &reqs[1],
        ),
        p.json(
            "the first model request of a brief run (system, messages, tools and every \
             other field of the body)",
            g::REQUEST_BRIEF_1_FIRST,
            first,
        ),
        p.json(
            "the second model request of a brief run: the first turn and its script's result",
            g::REQUEST_BRIEF_2_AFTER_A_SCRIPT,
            &after_the_first(first, &reqs[1].body),
        ),
        p.json(
            "the stored steps of a brief run",
            g::STEPS_BRIEF,
            &p.steps(first),
        ),
        p.json(
            "the audit records of a brief run",
            g::AUDIT_BRIEF,
            &p.audit(),
        ),
        p.ending_pinned(g::ENDING_FINISH_BRIEF),
        p.ending_net_pinned(g::ENDING_FINISH_BRIEF),
    ]);
}

/// A thread run that uses the web: a search, a fetch of its result, of a
/// page the sender named, of a link of that page as raw text, a second
/// window of a page from the run's store and a URL it was not given, then
/// a finish that lists its sources. Pins what the thread run's goldens do
/// not hold: a web call in a script step's `calls` (`cost_milli` and
/// `web`, which the run view reads), in the journal (its value or error,
/// `cost_milli`, `web`, `counted`, `source`, `format`), in the live
/// events (`call`, with `cost_milli` and `web`) and in the audit records
/// (`bot.web`).
#[tokio::test]
async fn a_web_run_is_pinned() {
    const LINKED: &str = "https://example.com/b";
    let env = Env::new().await;
    price_the_web(&env);
    let web = env.web();
    web.search_answers("blue headings", found());
    web.page(FOUND, fetched(FOUND));
    web.page(
        GIVEN,
        Ok(FetchedPage {
            final_url: GIVEN.into(),
            status: 200,
            content_type: "text/html".into(),
            text: "# Given\nSee the other page.".into(),
            links: vec![LINKED.into()],
            request_id: Some("req-2".into()),
        }),
    );
    web.page(LINKED, fetched(LINKED));
    sender_says(&env, &format!("Read {GIVEN}"));
    let code = format!(
        r#"const s = await clax.web.search("blue headings");
const found = await clax.web.fetch(s.results[0].url);
const given = await clax.web.fetch("{GIVEN}");
const linked = await clax.web.fetch("{LINKED}", {{format: "raw"}});
const again = await clax.web.fetch("{GIVEN}", {{offset: 8}});
let refused;
try {{ await clax.web.fetch("https://elsewhere.example/x"); }} catch (e) {{ refused = e.code; }}
return {{results: s.results.length, read: [found.totalChars, given.totalChars, linked.totalChars, again.text], refused, left: again.left}};"#
    );
    env.api.push(env.script_turn("tu_1", "Web", &code));
    env.api.push(finish_turn("Read three pages."));
    let run = env.run_with_web("search,fetch");
    let mut p = Pins::subscribed(env, run);
    p.drive().await;
    let reqs = p.env.api.requests();
    assert_eq!(reqs.len(), 2);
    let events = p.events().await;
    p.pin_all([
        p.json(
            "the stored steps of a run that used the web (a script's web calls, the finish's \
             sources)",
            g::STEPS_WEB,
            &p.steps(&reqs[0].body),
        ),
        p.json(
            "the journal of a run that used the web (each web call's result, cost, source and \
             format)",
            g::JOURNAL_WEB,
            &p.journal(),
        ),
        p.json(
            "the live events of a run that used the web (a web call's cost and what it \
             searched or fetched)",
            g::EVENTS_WEB,
            &events,
        ),
        p.json(
            "the audit records of a run that used the web (bot.web)",
            g::AUDIT_WEB,
            &p.audit(),
        ),
        p.ending_pinned(g::ENDING_FINISH_NO_REPLY_THREE_SOURCES),
        p.ending_net_pinned(g::ENDING_FINISH_NO_REPLY_THREE_SOURCES),
    ]);
}
