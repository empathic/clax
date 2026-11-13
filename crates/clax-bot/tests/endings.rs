//! What a run's end does beside what the goldens of `tests/golden/`
//! pin: a notice that cannot be posted, a `finish` whose sources
//! cannot be read, and a run whose tick died after its terminal
//! commit.

use crate::common::*;
use clax_bot::host::NOTICE_SEQ;
use clax_bot::run::CrashPoint;
use clax_bot::web::FetchedPage;
use clax_bot::{Driver, Next, Wait};
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use clax_store::roles::OWNER;
use std::sync::Arc;
use std::time::Duration;

/// Takes the table of the pages a run fetched away (`away`), so that
/// reading a run's sources fails, or puts it back.
pub fn pages_table(env: &Env, away: bool) {
    let db = clax_core::Home::at(env._dir.path().join("ax")).db_path();
    let c = rusqlite::Connection::open(&db).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    let (from, to) = if away {
        ("bot_web_pages", "bot_web_pages_away")
    } else {
        ("bot_web_pages_away", "bot_web_pages")
    };
    c.execute(&format!("ALTER TABLE {from} RENAME TO {to}"), [])
        .unwrap();
}

/// A run that fetched a page and is about to finish: its next tick
/// takes the `finish` step.
pub async fn about_to_finish_with_a_source(env: &Env) -> (String, DriverId) {
    const GIVEN: &str = "https://example.com/a";
    env.web().page(
        GIVEN,
        Ok(FetchedPage {
            final_url: GIVEN.into(),
            status: 200,
            content_type: "text/markdown".into(),
            text: "# Doc title\nbody".into(),
            links: Vec::new(),
            request_id: None,
        }),
    );
    env.host.lock().context.threads[0].comments[0].text = format!("Read {GIVEN}");
    env.api
        .push(env.script_turn("tu_1", "Web", &format!("await clax.web.fetch({GIVEN:?});")));
    env.api.push(finish_turn("Read the page."));
    let run = env.run_with_web("fetch");
    let (deps, driver) = (env.deps(), DriverId::mint());
    for kind in [StepKind::Turn, StepKind::Script, StepKind::Turn] {
        let out = env.tick(&deps, &run, &driver).await;
        assert_eq!((out.kind, out.next), (Some(kind), Next::Runnable));
    }
    (run, driver)
}

/// A notice that cannot be posted does not keep the run from ending:
/// the step commits with the notice's text, nothing is on the thread,
/// and the notice's place has no journal row.
#[tokio::test]
async fn a_notice_that_cannot_be_posted_leaves_the_run_ended_with_its_text() {
    let env = Env::new().await;
    let run = env.run();
    env.set(|s| s.enabled = false);
    // The person may no longer write: the reply is refused in its
    // transaction.
    env.host.lock().readers.push(OWNER.into());
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    assert_eq!(
        steps[0].payload.as_ref().unwrap()["notice"],
        "I stopped: ClaxBot is turned off on this Clax. I had not run any scripts yet."
    );
    assert!(env.replies().is_empty(), "{:?}", env.replies());
    let rows = env.store.bot_calls(&run, 1).unwrap();
    assert!(rows.iter().all(|c| c.seq() < NOTICE_SEQ), "{rows:?}");
    // The host was asked once, and the roster turned off once, after
    // the run ended.
    let host = env.host.lock();
    assert_eq!(
        host.calls,
        [(format!("{run}:1:{NOTICE_SEQ}"), "reply".to_string())]
    );
    let off: Vec<_> = host.roster.iter().filter(|c| !c.on).collect();
    assert_eq!(off.len(), 1);
    assert_eq!(off[0].status, Some(RunStatus::Stopped));
}

/// A `finish` whose sources cannot be read is tried again, each failure
/// counted against the step as a commit that failed, and nothing is
/// posted meanwhile. Once they can be read the run finishes, its
/// summary and sources posted once.
#[tokio::test]
async fn sources_that_cannot_be_read_are_a_counted_failure_of_the_finish_step() {
    let env = Env::new().await;
    let (run, driver) = about_to_finish_with_a_source(&env).await;
    let deps = env.deps();
    pages_table(&env, true);
    for attempt in 1..=2 {
        assert!(env.try_tick(&deps, &run, &driver, None).await.is_err());
        let open = env.store.open_step(&run).unwrap().unwrap();
        assert_eq!(
            (open.kind, open.attempt, open.failures),
            (StepKind::Finish, attempt, attempt)
        );
    }
    assert!(env.replies().is_empty());
    pages_table(&env, false);
    let out = env.tick(&deps, &run, &driver).await;
    assert_eq!(out.next, Next::Done(RunStatus::Finished));
    assert_eq!(
        env.replies(),
        [(
            env.t1.clone(),
            "Read the page.\n\nSources:\n- [Doc title](https://example.com/a)".to_string()
        )]
    );
}

/// Sources that cannot be read `max_commit_failures` times end the run
/// `failed`, as a step that cannot be committed does: the run's thread
/// is told so, and the summary is never posted. (Before a run's end
/// went through its surfaces the read was tried without end.)
#[tokio::test]
async fn sources_that_stay_unreadable_end_the_run_failed() {
    let env = Env::new().await;
    let (run, driver) = about_to_finish_with_a_source(&env).await;
    let deps = env.deps();
    pages_table(&env, true);
    let limit = env.settings.0.lock().unwrap().max_commit_failures;
    for _ in 0..limit {
        assert!(env.try_tick(&deps, &run, &driver, None).await.is_err());
    }
    let out = env.tick(&deps, &run, &driver).await;
    assert_eq!(out.next, Next::Done(RunStatus::Failed));
    let row = env.run_row(&run);
    assert_eq!(row.reason.as_deref(), Some("internal_error"));
    assert_eq!(
        env.replies(),
        [(
            env.t1.clone(),
            "I stopped: I could not save my work. I ran 1 script.".to_string()
        )]
    );
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    let last = steps.last().unwrap();
    assert_eq!((last.kind, last.failures), (StepKind::Failed, limit));
}

/// A tick that dies after its terminal commit has not turned the roster
/// off nor started the follow-ups. The next driver to start tells the
/// surface the run ended, for a run whose thread has a follow-up
/// queued: the host is told to stop showing the run, from which the
/// server starts the follow-up. The notice is on the thread once.
#[tokio::test]
async fn a_run_whose_tick_died_after_its_end_is_ended_for_its_surface_when_a_driver_starts() {
    let env = Env::new().await;
    let run = env.run();
    env.store
        .queue_follow_up_if_active(&env.t1, "someone-else")
        .unwrap()
        .expect("the run is at work on the thread");
    env.set(|s| s.enabled = false);
    let mut dies = env.deps();
    crate::resume::crash_once(
        &mut dies,
        CrashPoint::AfterCommit {
            step: 1,
            kind: StepKind::Stopped,
        },
    );
    let out = env.drive(&dies, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Waiting(Wait::Crashed));
    assert_eq!(env.run_row(&run).status, RunStatus::Stopped);
    assert!(env.host.lock().roster.is_empty(), "the tick died first");
    let driver = Driver::new(DriverId::mint(), Arc::new(env.deps()), env.hub.clone());
    driver.start(true).await.unwrap();
    let roster = env.host.lock().roster.clone();
    assert_eq!(roster.len(), 1, "{roster:?}");
    assert_eq!(
        (roster[0].on, roster[0].status),
        (false, Some(RunStatus::Stopped))
    );
    assert_eq!(env.replies().len(), 1, "the notice is posted once");
}

/// A run that ended and was told so awaits nothing: a driver that
/// starts later calls no one.
#[tokio::test]
async fn a_driver_that_starts_tells_no_surface_of_a_run_with_nothing_left_to_do() {
    let env = Env::new().await;
    let run = env.run();
    env.set(|s| s.enabled = false);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    let before = env.host.lock().roster.len();
    let driver = Driver::new(DriverId::mint(), Arc::new(env.deps()), env.hub.clone());
    driver.start(true).await.unwrap();
    assert_eq!(env.host.lock().roster.len(), before);
}

/// Every field at the top of a script step's and a terminal step's
/// payload is one of the run loop's own or one of those `clax`'s
/// provider is listed as adding, which a host's surface may not be
/// named as.
#[tokio::test]
async fn a_steps_fields_are_the_run_loops_and_the_ones_listed_as_claxs() {
    use clax_bot::surface::{CLAX_FIELDS, PAYLOAD_KEYS};
    let mut seen = std::collections::BTreeSet::new();
    for stopped in [false, true] {
        let env = Env::new().await;
        let (run, driver) = if stopped {
            env.set(|s| s.enabled = false);
            (env.run(), DriverId::mint())
        } else {
            about_to_finish_with_a_source(&env).await
        };
        env.drive(&env.deps(), &run, &driver).await;
        let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
        for step in steps.iter().filter(|s| s.kind != StepKind::Turn) {
            for name in step.payload.as_ref().unwrap().as_object().unwrap().keys() {
                assert!(
                    PAYLOAD_KEYS.contains(&name.as_str()) || CLAX_FIELDS.contains(&name.as_str()),
                    "{name} in a {:?} step",
                    step.kind
                );
                seen.insert(name.clone());
            }
        }
    }
    for name in CLAX_FIELDS {
        assert!(seen.contains(name), "no step of these runs has `{name}`");
    }
}
